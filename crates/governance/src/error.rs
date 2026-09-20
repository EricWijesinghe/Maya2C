//! Why a governance operation was refused.

use core::fmt;

use crate::params::ParameterKey;
use crate::proposal::ProposalState;

/// A proposal, a vote, or an execution could not proceed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GovernanceError {
    /// A proposal named a parameter this build does not implement.
    ///
    /// Refused rather than skipped. A node that ignored an unknown key would
    /// execute a proposal it did not understand and then build on a state every
    /// upgraded node disagrees with — a silent fork. Refusing turns that into a
    /// stopped node, which an operator can see.
    UnknownParameter {
        /// The tag that was not recognised.
        tag: u16,
    },
    /// A proposed value is outside the range its parameter permits.
    ParameterOutOfRange {
        /// Parameter involved.
        key: ParameterKey,
        /// Value that was proposed.
        value: u64,
        /// Smallest permitted.
        min: u64,
        /// Largest permitted.
        max: u64,
    },
    /// A proposal carried no changes, or more than
    /// [`crate::params::MAX_CHANGES_PER_PROPOSAL`].
    ChangeCountOutOfRange {
        /// How many were carried.
        count: usize,
    },
    /// A proposal named one parameter twice.
    ///
    /// Two values for one key have no defined order of application, so the
    /// result would depend on iteration rather than on the vote.
    DuplicateParameter {
        /// The repeated key.
        key: ParameterKey,
    },
    /// A voting period or timelock is outside what [`crate::limits`] permits.
    ///
    /// These bounds are compiled in and no proposal can move them; see the
    /// module documentation for why.
    ScheduleOutOfRange {
        /// Which schedule field.
        what: &'static str,
        /// The value supplied.
        blocks: u64,
    },
    /// An operation was attempted in a state that does not admit it.
    WrongState {
        /// What was attempted.
        action: &'static str,
        /// The state the proposal is actually in.
        state: ProposalState,
    },
    /// A vote arrived outside the proposal's voting window.
    OutsideVotingWindow {
        /// Height the vote arrived at.
        height: u64,
        /// Height voting closes at, inclusive.
        closes: u64,
    },
    /// Execution was attempted before the timelock elapsed.
    ///
    /// The exit window: its purpose is to give everyone who disagrees time to
    /// act before the change binds them.
    TimelockNotElapsed {
        /// Height execution was attempted at.
        height: u64,
        /// Earliest height it may execute.
        earliest: u64,
    },
    /// A passed proposal was left unexecuted past its grace period.
    ///
    /// Distinct from being rejected. It had the votes; nobody executed it in
    /// time, and a stale change must not land into a chain that has moved on.
    Expired {
        /// Height execution was attempted at.
        height: u64,
        /// Height after which it could no longer execute.
        deadline: u64,
    },
    /// A voter with no weight tried to vote.
    ///
    /// Refused rather than counted as zero, so a zero-weight vote cannot be
    /// used to pad turnout toward a quorum.
    NoVotingPower,
    /// The same address voted twice on one proposal.
    AlreadyVoted,
    /// Vote weights overflowed the tally.
    TallyOverflow,
    /// A lock would expire before the proposal it is voting on can execute.
    ///
    /// The vote-then-sell guard. Without it, the cheapest way to decide
    /// something is to acquire weight, vote, and be gone before the decision
    /// binds anyone.
    LockExpiresTooSoon {
        /// Height the lock releases at.
        unlock_height: u64,
        /// Height it must survive to.
        required: u64,
    },
}

impl fmt::Display for GovernanceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownParameter { tag } => {
                write!(f, "unknown governance parameter tag {tag}")
            }
            Self::ParameterOutOfRange {
                key,
                value,
                min,
                max,
            } => write!(
                f,
                "{} = {value} is outside the permitted range [{min}, {max}]",
                key.label()
            ),
            Self::ChangeCountOutOfRange { count } => {
                write!(f, "a proposal carrying {count} changes is not permitted")
            }
            Self::DuplicateParameter { key } => {
                write!(f, "{} is named twice in one proposal", key.label())
            }
            Self::ScheduleOutOfRange { what, blocks } => {
                write!(
                    f,
                    "{what} of {blocks} blocks is outside the permitted range"
                )
            }
            Self::WrongState { action, state } => {
                write!(f, "cannot {action} a proposal that is {}", state.label())
            }
            Self::OutsideVotingWindow { height, closes } => {
                write!(f, "voting closed at height {closes}; this is {height}")
            }
            Self::TimelockNotElapsed { height, earliest } => {
                write!(f, "timelock runs to height {earliest}; this is {height}")
            }
            Self::Expired { height, deadline } => {
                write!(
                    f,
                    "execution expired at height {deadline}; this is {height}"
                )
            }
            Self::NoVotingPower => f.write_str("the voter holds no voting power"),
            Self::AlreadyVoted => f.write_str("this address has already voted"),
            Self::TallyOverflow => f.write_str("vote weights exceeded the tally"),
            Self::LockExpiresTooSoon {
                unlock_height,
                required,
            } => write!(
                f,
                "the stake unlocks at height {unlock_height} but must be held to {required}"
            ),
        }
    }
}

impl core::error::Error for GovernanceError {}

/// Convenience alias for this crate's fallible operations.
pub type Result<T> = core::result::Result<T, GovernanceError>;
