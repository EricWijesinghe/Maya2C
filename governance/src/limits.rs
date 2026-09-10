//! The bounds governance may never escape.
//!
//! # Why a governed chain needs ungoverned floors
//!
//! Governance that can change anything can break everything, and the first move
//! of a hostile majority is not to steal — it is to remove the checks that would
//! have let anyone react. Set the timelock to zero and the exit window
//! disappears; set the quorum to one and the majority stops needing to be one.
//!
//! So the parameters that protect the process are **not proposable at all**.
//! They are compiled into the binary, they appear in no
//! [`crate::params::ParameterKey`], and there is no transaction that changes
//! them. Changing them is a release and a hard fork, which is to say it is a
//! decision every node operator makes individually by choosing what to run.
//!
//! That is a real limitation and it is the point: a chain whose safety margins
//! can be voted away has margins only until somebody wants them gone.
//!
//! # Everything else is range-checked
//!
//! Parameters that *are* proposable carry a hard `[min, max]` in
//! [`crate::params`], checked when the proposal is made and **again** when it
//! executes. Twice, because a range could be tightened by a release between
//! those two moments, and a value that was legal when proposed must not become
//! law after it stopped being legal.

/// Fewest blocks a proposal must remain open for voting.
///
/// 480 blocks — about two hours at the 15-second target in
/// `consensus::difficulty::TARGET_BLOCK_TIME`. Short enough that governance is
/// usable, long enough that a vote cannot be opened and closed between two
/// glances at a block explorer.
pub const MIN_VOTING_BLOCKS: u64 = 480;

/// Most blocks a proposal may remain open.
///
/// A ceiling as well as a floor, because a proposal open for a year is a
/// proposal whose voters have long since changed their minds and whose locked
/// stake is stranded behind it.
pub const MAX_VOTING_BLOCKS: u64 = 100_000;

/// Fewest blocks between a proposal passing and executing.
///
/// The exit window. Its entire purpose is to give everyone who disagrees with a
/// passed proposal time to act before it binds them — sell, withdraw, fork, or
/// simply stop running the release. A timelock of zero is a governance system
/// with no dissent mechanism, which is why this is a floor rather than a
/// default.
///
/// 960 blocks, about four hours.
pub const MIN_TIMELOCK_BLOCKS: u64 = 960;

/// Most blocks a proposal may wait between passing and executing.
///
/// Bounded so a passed proposal cannot sit indefinitely and execute into a
/// chain that has moved on. Past this it expires unexecuted.
pub const MAX_TIMELOCK_BLOCKS: u64 = 200_000;

/// Least turnout, in basis points of eligible stake, for a vote to count.
///
/// Ten percent. Not a claim that ten percent is enough participation to be
/// legitimate — that is a social question — but the point below which a result
/// is plainly an artefact of who happened to be awake.
pub const MIN_QUORUM_BPS: u32 = 1_000;

/// Least share of decisive votes, in basis points, needed to pass.
///
/// 5001: a strict majority, and nothing less is ever permitted. A threshold at
/// or below half admits a proposal and its opposite both passing, which is not
/// a close vote — it is two contradictory laws.
pub const MIN_APPROVAL_BPS: u32 = 5_001;

/// Basis-point denominator.
pub const BPS_DENOMINATOR: u32 = 10_000;

/// Most blocks a passed proposal may wait to be executed before expiring.
///
/// Distinct from the timelock: the timelock is how long it *must* wait, this is
/// how long it *may* be executed for afterwards. Without it, a proposal that
/// nobody executed stays executable forever, and a stale change lands years
/// later into a chain nobody expected it in.
pub const EXECUTION_GRACE_BLOCKS: u64 = 100_000;

/// Whether a voting period is permissible.
#[must_use]
pub const fn is_voting_period_valid(blocks: u64) -> bool {
    blocks >= MIN_VOTING_BLOCKS && blocks <= MAX_VOTING_BLOCKS
}

/// Whether a timelock is permissible.
#[must_use]
pub const fn is_timelock_valid(blocks: u64) -> bool {
    blocks >= MIN_TIMELOCK_BLOCKS && blocks <= MAX_TIMELOCK_BLOCKS
}

/// Whether a quorum threshold is permissible.
#[must_use]
pub const fn is_quorum_valid(bps: u32) -> bool {
    bps >= MIN_QUORUM_BPS && bps <= BPS_DENOMINATOR
}

/// Whether an approval threshold is permissible.
#[must_use]
pub const fn is_approval_valid(bps: u32) -> bool {
    bps >= MIN_APPROVAL_BPS && bps <= BPS_DENOMINATOR
}
