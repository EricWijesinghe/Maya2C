//! The proposal lifecycle.
//!
//! ```text
//!            ┌──────────► Cancelled          (proposer withdraws, before close)
//!            │
//!  Voting ───┼──────────► Rejected           (quorum missed, or approval missed)
//!            │
//!            └──► Queued ─┬──► Executed      (timelock elapsed, grace not)
//!                         └──► Expired       (grace elapsed, nobody executed)
//! ```
//!
//! # Every transition is a function of height
//!
//! Nothing here happens because time passed; it happens because a block at some
//! height asked. Two nodes replaying the same chain therefore make the same
//! transitions in the same blocks, which is the only property that matters —
//! a proposal that executed on one node and not another is a fork.
//!
//! # Why `Queued` is a state and not a delay
//!
//! A passed proposal does not execute. It becomes executable, later. The gap is
//! the exit window, and its purpose is to give everyone who disagrees time to
//! act before the change binds them: sell, withdraw, fork, or simply decline to
//! run the release. `MIN_TIMELOCK_BLOCKS` is a floor rather than a default for
//! exactly that reason — a governance system whose timelock can reach zero has
//! no dissent mechanism at all.
//!
//! # Why `Expired` exists
//!
//! A proposal that passed and was never executed must not stay executable
//! forever. Otherwise a change nobody remembers wanting lands years later into
//! a chain it was never argued about.

use crate::error::{GovernanceError, Result};
use crate::limits::{EXECUTION_GRACE_BLOCKS, is_timelock_valid, is_voting_period_valid};
use crate::tally::{Choice, Tally};

/// Where a proposal is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProposalState {
    /// Open for votes.
    Voting,
    /// Passed, waiting out the timelock.
    Queued,
    /// Applied.
    Executed,
    /// Failed its thresholds.
    Rejected,
    /// Withdrawn by its proposer before voting closed.
    Cancelled,
    /// Passed, but nobody executed it in time.
    Expired,
}

impl ProposalState {
    /// Stable wire tag. Never renumber — it is consensus.
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::Voting => 1,
            Self::Queued => 2,
            Self::Executed => 3,
            Self::Rejected => 4,
            Self::Cancelled => 5,
            Self::Expired => 6,
        }
    }

    /// Decodes a wire tag.
    #[must_use]
    pub const fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            1 => Some(Self::Voting),
            2 => Some(Self::Queued),
            3 => Some(Self::Executed),
            4 => Some(Self::Rejected),
            5 => Some(Self::Cancelled),
            6 => Some(Self::Expired),
            _ => None,
        }
    }

    /// Whether this state can never change again.
    ///
    /// A settled proposal still holds its record — the history is the point —
    /// but nothing further happens to it, and its proposer's deposit is
    /// releasable.
    #[must_use]
    pub const fn is_settled(self) -> bool {
        matches!(
            self,
            Self::Executed | Self::Rejected | Self::Cancelled | Self::Expired
        )
    }

    /// Short label for errors and explorers.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Voting => "open for voting",
            Self::Queued => "queued behind its timelock",
            Self::Executed => "executed",
            Self::Rejected => "rejected",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired unexecuted",
        }
    }
}

/// The heights that govern one proposal.
///
/// Computed once at creation and never recomputed. Deriving them at each
/// transition would mean deriving them identically every time, and a schedule
/// that shifted under a proposal is a schedule voters did not agree to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Schedule {
    /// Height the proposal was created at.
    pub created: u64,
    /// Last height at which a vote is accepted, inclusive.
    pub voting_closes: u64,
    /// Earliest height a passed proposal may execute at.
    pub executable_from: u64,
    /// Last height a passed proposal may execute at, inclusive.
    pub expires_after: u64,
}

/// A proposal, as the state machine sees it.
///
/// Carries no identifier, no author, and no changes: those are chain records,
/// and this crate is deliberately storage-free. What it carries is everything
/// needed to decide what may happen next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Proposal {
    /// Where it is in its life.
    pub state: ProposalState,
    /// The heights that govern it.
    pub schedule: Schedule,
    /// Votes so far.
    pub tally: Tally,
}

impl Proposal {
    /// Opens a proposal at `height`.
    ///
    /// # Errors
    ///
    /// Returns [`GovernanceError::ScheduleOutOfRange`] if the voting period or
    /// the timelock is outside what [`crate::limits`] permits. Those bounds are
    /// compiled in and no proposal can move them.
    pub const fn open(height: u64, voting_blocks: u64, timelock_blocks: u64) -> Result<Self> {
        if !is_voting_period_valid(voting_blocks) {
            return Err(GovernanceError::ScheduleOutOfRange {
                what: "voting period",
                blocks: voting_blocks,
            });
        }
        if !is_timelock_valid(timelock_blocks) {
            return Err(GovernanceError::ScheduleOutOfRange {
                what: "timelock",
                blocks: timelock_blocks,
            });
        }

        // Saturating throughout. A proposal opened near `u64::MAX` would be a
        // proposal whose schedule wrapped to the past and became immediately
        // executable, which is the one arithmetic failure here that is worth
        // anything to an attacker.
        let voting_closes = height.saturating_add(voting_blocks);
        let executable_from = voting_closes.saturating_add(timelock_blocks);
        let expires_after = executable_from.saturating_add(EXECUTION_GRACE_BLOCKS);

        Ok(Self {
            state: ProposalState::Voting,
            schedule: Schedule {
                created: height,
                voting_closes,
                executable_from,
                expires_after,
            },
            tally: Tally::empty(),
        })
    }

    /// The height a voter's stake must remain locked until.
    ///
    /// Execution, not the close of voting. The vote-then-sell guard: without
    /// it, the cheapest way to decide something is to acquire weight, vote, and
    /// be gone before the decision binds anyone who stayed.
    #[must_use]
    pub const fn lock_required_until(&self) -> u64 {
        self.schedule.executable_from
    }

    /// Records a vote.
    ///
    /// # Errors
    ///
    /// - [`GovernanceError::WrongState`] unless the proposal is open.
    /// - [`GovernanceError::OutsideVotingWindow`] past the close.
    /// - Whatever [`Tally::add`] returns for the weight.
    pub const fn vote(&self, choice: Choice, weight: u128, height: u64) -> Result<Self> {
        if !matches!(self.state, ProposalState::Voting) {
            return Err(GovernanceError::WrongState {
                action: "vote on",
                state: self.state,
            });
        }
        if height > self.schedule.voting_closes {
            return Err(GovernanceError::OutsideVotingWindow {
                height,
                closes: self.schedule.voting_closes,
            });
        }

        let tally = match self.tally.add(choice, weight) {
            Ok(tally) => tally,
            Err(error) => return Err(error),
        };
        Ok(Self { tally, ..*self })
    }

    /// Whether voting has closed at `height`.
    #[must_use]
    pub const fn voting_closed(&self, height: u64) -> bool {
        height > self.schedule.voting_closes
    }

    /// Closes voting and moves to [`ProposalState::Queued`] or
    /// [`ProposalState::Rejected`].
    ///
    /// Idempotent in effect: a proposal already past `Voting` is returned
    /// unchanged rather than refused, because the end-of-block pass that calls
    /// this visits every open proposal and must not fail on one that a
    /// cancellation already settled.
    ///
    /// # Errors
    ///
    /// Returns [`GovernanceError::WrongState`] if called before voting closes.
    /// Finalizing early would decide a vote that is still being cast.
    pub const fn finalize(
        &self,
        height: u64,
        eligible: u128,
        quorum_bps: u32,
        approval_bps: u32,
    ) -> Result<Self> {
        if !matches!(self.state, ProposalState::Voting) {
            return Ok(*self);
        }
        if !self.voting_closed(height) {
            return Err(GovernanceError::WrongState {
                action: "finalize",
                state: self.state,
            });
        }

        let state = if self.tally.passes(eligible, quorum_bps, approval_bps) {
            ProposalState::Queued
        } else {
            ProposalState::Rejected
        };
        Ok(Self { state, ..*self })
    }

    /// Whether a queued proposal may execute at `height`.
    #[must_use]
    pub const fn is_executable(&self, height: u64) -> bool {
        matches!(self.state, ProposalState::Queued)
            && height >= self.schedule.executable_from
            && height <= self.schedule.expires_after
    }

    /// Marks a queued proposal executed.
    ///
    /// # Errors
    ///
    /// - [`GovernanceError::WrongState`] unless it is queued.
    /// - [`GovernanceError::TimelockNotElapsed`] before the exit window closes.
    /// - [`GovernanceError::Expired`] past the grace period.
    pub const fn execute(&self, height: u64) -> Result<Self> {
        if !matches!(self.state, ProposalState::Queued) {
            return Err(GovernanceError::WrongState {
                action: "execute",
                state: self.state,
            });
        }
        if height < self.schedule.executable_from {
            return Err(GovernanceError::TimelockNotElapsed {
                height,
                earliest: self.schedule.executable_from,
            });
        }
        if height > self.schedule.expires_after {
            return Err(GovernanceError::Expired {
                height,
                deadline: self.schedule.expires_after,
            });
        }

        Ok(Self {
            state: ProposalState::Executed,
            ..*self
        })
    }

    /// Marks a queued proposal expired.
    ///
    /// Returned unchanged unless it is genuinely past its grace period, so the
    /// end-of-block pass can call this on everything queued.
    #[must_use]
    pub const fn expire_if_stale(&self, height: u64) -> Self {
        if matches!(self.state, ProposalState::Queued) && height > self.schedule.expires_after {
            return Self {
                state: ProposalState::Expired,
                ..*self
            };
        }
        *self
    }

    /// Withdraws a proposal before voting closes.
    ///
    /// Only while open. A proposal that has passed belongs to everyone who
    /// voted for it, and letting its author retract it afterwards would make
    /// every vote conditional on the author's continued agreement.
    ///
    /// # Errors
    ///
    /// Returns [`GovernanceError::WrongState`] if voting has closed.
    pub const fn cancel(&self) -> Result<Self> {
        if !matches!(self.state, ProposalState::Voting) {
            return Err(GovernanceError::WrongState {
                action: "cancel",
                state: self.state,
            });
        }
        Ok(Self {
            state: ProposalState::Cancelled,
            ..*self
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::{MIN_TIMELOCK_BLOCKS, MIN_VOTING_BLOCKS};

    fn open() -> Proposal {
        Proposal::open(100, MIN_VOTING_BLOCKS, MIN_TIMELOCK_BLOCKS).expect("open")
    }

    #[test]
    fn a_schedule_below_the_compiled_floors_is_refused() {
        // These floors are the reason governance cannot make itself unsafe, so
        // the first thing worth checking is that they are not advisory.
        assert!(Proposal::open(1, MIN_VOTING_BLOCKS - 1, MIN_TIMELOCK_BLOCKS).is_err());
        assert!(Proposal::open(1, MIN_VOTING_BLOCKS, MIN_TIMELOCK_BLOCKS - 1).is_err());
        assert!(Proposal::open(1, 0, 0).is_err());
        assert!(Proposal::open(1, MIN_VOTING_BLOCKS, MIN_TIMELOCK_BLOCKS).is_ok());
    }

    #[test]
    fn a_proposal_opened_near_the_end_of_time_does_not_wrap_into_the_past() {
        // Wrapping would produce a proposal that is executable the moment it is
        // created, which is the one arithmetic failure here worth anything to
        // an attacker.
        let proposal =
            Proposal::open(u64::MAX - 1, MIN_VOTING_BLOCKS, MIN_TIMELOCK_BLOCKS).expect("open");

        assert!(proposal.schedule.voting_closes >= proposal.schedule.created);
        assert!(proposal.schedule.executable_from >= proposal.schedule.voting_closes);
        assert!(proposal.schedule.expires_after >= proposal.schedule.executable_from);
        assert!(!proposal.is_executable(u64::MAX));
    }

    #[test]
    fn a_vote_after_the_close_is_refused() {
        let proposal = open();
        let closes = proposal.schedule.voting_closes;

        assert!(proposal.vote(Choice::For, 1, closes).is_ok());
        assert_eq!(
            proposal.vote(Choice::For, 1, closes + 1),
            Err(GovernanceError::OutsideVotingWindow {
                height: closes + 1,
                closes
            })
        );
    }

    #[test]
    fn finalizing_before_the_close_is_refused() {
        // Finalizing early decides a vote that is still being cast.
        let proposal = open();
        assert!(matches!(
            proposal.finalize(proposal.schedule.voting_closes, 1_000, 1_000, 5_001),
            Err(GovernanceError::WrongState { .. })
        ));
    }

    #[test]
    fn a_passing_vote_queues_rather_than_executes() {
        // The gap between passing and taking effect is the exit window, and it
        // is the whole reason a timelock exists.
        let proposal = open()
            .vote(Choice::For, 800, 200)
            .expect("vote")
            .finalize(1_000, 1_000, 1_000, 5_001)
            .expect("finalize");

        assert_eq!(proposal.state, ProposalState::Queued);
        assert!(!proposal.is_executable(proposal.schedule.voting_closes));
        assert!(proposal.is_executable(proposal.schedule.executable_from));
    }

    #[test]
    fn executing_before_the_timelock_elapses_is_refused() {
        let queued = open()
            .vote(Choice::For, 800, 200)
            .expect("vote")
            .finalize(1_000, 1_000, 1_000, 5_001)
            .expect("finalize");

        let early = queued.schedule.executable_from - 1;
        assert_eq!(
            queued.execute(early),
            Err(GovernanceError::TimelockNotElapsed {
                height: early,
                earliest: queued.schedule.executable_from,
            })
        );
        assert!(queued.execute(queued.schedule.executable_from).is_ok());
    }

    #[test]
    fn a_proposal_that_misses_quorum_is_rejected_however_unanimous() {
        // Unanimity among three voters is not a mandate when a thousand were
        // eligible.
        let proposal = open()
            .vote(Choice::For, 3, 200)
            .expect("vote")
            .finalize(1_000, 1_000, 1_000, 5_001)
            .expect("finalize");

        assert_eq!(proposal.state, ProposalState::Rejected);
    }

    #[test]
    fn a_proposal_that_misses_approval_is_rejected_however_well_attended() {
        let proposal = open()
            .vote(Choice::For, 500, 200)
            .expect("for")
            .vote(Choice::Against, 500, 200)
            .expect("against")
            .finalize(1_000, 1_000, 1_000, 5_001)
            .expect("finalize");

        assert_eq!(proposal.state, ProposalState::Rejected);
    }

    #[test]
    fn a_passed_proposal_expires_if_nobody_executes_it() {
        // Otherwise a change nobody remembers wanting lands years later into a
        // chain it was never argued about.
        let queued = open()
            .vote(Choice::For, 800, 200)
            .expect("vote")
            .finalize(1_000, 1_000, 1_000, 5_001)
            .expect("finalize");

        let past = queued.schedule.expires_after + 1;
        assert_eq!(
            queued.execute(past),
            Err(GovernanceError::Expired {
                height: past,
                deadline: queued.schedule.expires_after,
            })
        );
        assert_eq!(queued.expire_if_stale(past).state, ProposalState::Expired);
        assert_eq!(
            queued.expire_if_stale(queued.schedule.expires_after).state,
            ProposalState::Queued
        );
    }

    #[test]
    fn a_proposal_cannot_be_withdrawn_once_it_has_passed() {
        // A passed proposal belongs to everyone who voted for it. Letting its
        // author retract it would make every vote conditional on the author's
        // continued agreement.
        let queued = open()
            .vote(Choice::For, 800, 200)
            .expect("vote")
            .finalize(1_000, 1_000, 1_000, 5_001)
            .expect("finalize");

        assert!(matches!(
            queued.cancel(),
            Err(GovernanceError::WrongState { .. })
        ));
        assert!(open().cancel().is_ok());
    }

    #[test]
    fn a_settled_proposal_admits_nothing_further() {
        let rejected = open()
            .finalize(1_000, 1_000, 1_000, 5_001)
            .expect("finalize");
        assert!(rejected.state.is_settled());

        assert!(rejected.vote(Choice::For, 1, 200).is_err());
        assert!(rejected.execute(100_000).is_err());
        assert!(rejected.cancel().is_err());
        // Finalizing again is a no-op rather than an error: the end-of-block
        // pass visits every proposal and must not fail on a settled one.
        assert_eq!(rejected.finalize(2_000, 1_000, 1_000, 5_001), Ok(rejected));
    }

    #[test]
    fn the_lock_must_outlive_the_timelock_not_merely_the_vote() {
        // The vote-then-sell guard, stated as an equality rather than left to
        // the caller to derive.
        let proposal = open();
        assert_eq!(
            proposal.lock_required_until(),
            proposal.schedule.executable_from
        );
        assert!(proposal.lock_required_until() > proposal.schedule.voting_closes);
    }
}
