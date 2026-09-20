//! The compiled floors, and what a refusal says.
//!
//! These are the values no proposal can reach, so a test that they are actually
//! enforced is a test of the one property the whole subsystem rests on: a
//! governance system that can vote away its own quorum or timelock has neither.

use maya_governance::error::GovernanceError;
use maya_governance::limits::{
    BPS_DENOMINATOR, EXECUTION_GRACE_BLOCKS, MAX_TIMELOCK_BLOCKS, MAX_VOTING_BLOCKS,
    MIN_APPROVAL_BPS, MIN_QUORUM_BPS, MIN_TIMELOCK_BLOCKS, MIN_VOTING_BLOCKS, is_approval_valid,
    is_quorum_valid, is_timelock_valid, is_voting_period_valid,
};
use maya_governance::params::ParameterKey;
use maya_governance::proposal::ProposalState;

#[test]
fn a_voting_period_is_valid_exactly_within_its_range() {
    assert!(!is_voting_period_valid(0));
    assert!(!is_voting_period_valid(MIN_VOTING_BLOCKS - 1));
    assert!(is_voting_period_valid(MIN_VOTING_BLOCKS));
    assert!(is_voting_period_valid(MAX_VOTING_BLOCKS));
    assert!(!is_voting_period_valid(MAX_VOTING_BLOCKS + 1));
    assert!(!is_voting_period_valid(u64::MAX));
}

#[test]
fn a_timelock_is_valid_exactly_within_its_range() {
    // The floor is the exit window: time for everyone who disagrees to act
    // before the change binds them. Zero is the value a hostile majority wants.
    assert!(!is_timelock_valid(0));
    assert!(!is_timelock_valid(MIN_TIMELOCK_BLOCKS - 1));
    assert!(is_timelock_valid(MIN_TIMELOCK_BLOCKS));
    assert!(is_timelock_valid(MAX_TIMELOCK_BLOCKS));
    assert!(!is_timelock_valid(MAX_TIMELOCK_BLOCKS + 1));
}

#[test]
fn a_quorum_below_the_floor_or_above_certainty_is_refused() {
    assert!(!is_quorum_valid(0));
    assert!(!is_quorum_valid(MIN_QUORUM_BPS - 1));
    assert!(is_quorum_valid(MIN_QUORUM_BPS));
    assert!(is_quorum_valid(BPS_DENOMINATOR));
    // Above 100% is not a strict quorum, it is an unreachable one — a
    // governance system that can never act.
    assert!(!is_quorum_valid(BPS_DENOMINATOR + 1));
}

#[test]
fn an_approval_threshold_must_be_a_strict_majority() {
    // At or below half, a proposal and its opposite can both pass. That is not
    // a close vote, it is two contradictory laws.
    assert!(!is_approval_valid(5_000));
    assert!(!is_approval_valid(MIN_APPROVAL_BPS - 1));
    assert!(is_approval_valid(MIN_APPROVAL_BPS));
    assert!(is_approval_valid(BPS_DENOMINATOR));
    assert!(!is_approval_valid(BPS_DENOMINATOR + 1));
}

#[test]
fn the_floors_are_ordered_so_a_valid_schedule_exists() {
    // A floor above its own ceiling would make every proposal unopenable — a
    // governance system that had quietly turned itself off.
    //
    // Asserted in a `const` block, so an edit that inverted one of these fails
    // to *compile* rather than failing a test somebody might not have run.
    // These are the values that make governance safe; a build that produces
    // them wrong should not produce a binary at all.
    const {
        assert!(MIN_VOTING_BLOCKS <= MAX_VOTING_BLOCKS);
        assert!(MIN_TIMELOCK_BLOCKS <= MAX_TIMELOCK_BLOCKS);
        assert!(MIN_QUORUM_BPS <= BPS_DENOMINATOR);
        assert!(MIN_APPROVAL_BPS <= BPS_DENOMINATOR);
        assert!(EXECUTION_GRACE_BLOCKS > 0);
        // A strict majority, always. At or below half, a proposal and its
        // opposite can both pass.
        assert!(MIN_APPROVAL_BPS > BPS_DENOMINATOR / 2);
    }
}

#[test]
fn every_refusal_says_something_specific() {
    // A refusal that renders as an empty string, or as the same string as
    // another refusal, is a log line an operator cannot act on.
    let all = [
        GovernanceError::UnknownParameter { tag: 99 },
        GovernanceError::ParameterOutOfRange {
            key: ParameterKey::DexProtocolFeeBps,
            value: 9_999,
            min: 0,
            max: 200,
        },
        GovernanceError::ChangeCountOutOfRange { count: 0 },
        GovernanceError::DuplicateParameter {
            key: ParameterKey::VmMaxMemoryPages,
        },
        GovernanceError::ScheduleOutOfRange {
            what: "timelock",
            blocks: 1,
        },
        GovernanceError::WrongState {
            action: "execute",
            state: ProposalState::Rejected,
        },
        GovernanceError::OutsideVotingWindow {
            height: 10,
            closes: 5,
        },
        GovernanceError::TimelockNotElapsed {
            height: 10,
            earliest: 20,
        },
        GovernanceError::Expired {
            height: 30,
            deadline: 20,
        },
        GovernanceError::NoVotingPower,
        GovernanceError::AlreadyVoted,
        GovernanceError::TallyOverflow,
        GovernanceError::LockExpiresTooSoon {
            unlock_height: 5,
            required: 10,
        },
    ];

    let mut seen = std::collections::HashSet::new();
    for error in all {
        let text = error.to_string();
        assert!(!text.is_empty(), "{error:?} renders as nothing");
        assert!(seen.insert(text), "{error:?} shares a message with another");
    }
}

#[test]
fn a_range_refusal_names_the_range_it_would_have_accepted() {
    // The rejection has to say what would have been acceptable, or a proposer
    // is left guessing at a bound they cannot see.
    let error = ParameterKey::DexProtocolFeeBps
        .check(9_999)
        .expect_err("out of range");
    let text = error.to_string();

    assert!(text.contains("dex.protocol_fee_bps"));
    assert!(text.contains("9999"));
    assert!(text.contains("200"), "the ceiling is not in the message");
}

#[test]
fn every_proposal_state_round_trips_and_settles_correctly() {
    let states = [
        (ProposalState::Voting, false),
        (ProposalState::Queued, false),
        (ProposalState::Executed, true),
        (ProposalState::Rejected, true),
        (ProposalState::Cancelled, true),
        (ProposalState::Expired, true),
    ];

    for (state, settled) in states {
        assert_eq!(ProposalState::from_tag(state.tag()), Some(state));
        assert_eq!(state.is_settled(), settled, "{}", state.label());
        assert!(!state.label().is_empty());
    }

    for tag in [0u8, 7, 255] {
        assert_eq!(ProposalState::from_tag(tag), None);
    }
}
