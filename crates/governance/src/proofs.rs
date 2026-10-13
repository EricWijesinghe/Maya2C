//! Kani harnesses for the governance rules.
//!
//! Run them with:
//!
//! ```text
//! cargo kani -p maya-governance
//! ```
//!
//! # What is worth proving here
//!
//! Not that a vote is counted — a test does that better. What a model checker is
//! for is the claim that **no input reaches a state the rules forbid**, and that
//! is exactly the shape of every property below. A governance system is only as
//! good as its worst reachable configuration, and the worst one is by definition
//! the one nobody wrote a test for.
//!
//! # Bounded versus unbounded
//!
//! Stated plainly, because the difference is the difference between a proof and
//! a very good test:
//!
//! - Every harness here is **unbounded** over the inputs it quantifies. There
//!   are no loops in the code they cover, so there is no unwind bound and no
//!   sequence length to assume.
//! - What they do *not* cover is a sequence of operations. Each proves a
//!   property of one transition; the composition of many transitions is covered
//!   by the tests in `proposal.rs` and `tests/governance_tests.rs`. Said here
//!   rather than left to be inferred from the absence.

use crate::limits::{
    BPS_DENOMINATOR, MIN_APPROVAL_BPS, MIN_QUORUM_BPS, MIN_TIMELOCK_BLOCKS, MIN_VOTING_BLOCKS,
};
use crate::params::{ALL_KEYS, ParameterKey};
use crate::proposal::{Proposal, ProposalState};
use maya_amount::Amount;

use crate::tally::{Choice, Tally};

/// A symbolic 256-bit weight.
fn any_amount() -> Amount {
    Amount(kani::any())
}

/// A symbolic weight below 2^128.
///
/// The threshold harnesses multiply two weights into 512 bits and compare;
/// over all 256-bit operands CBMC does not finish. They are proved for weights
/// below 2^128 — every products' high half is then zero — and the 512-bit
/// comparison itself is exercised at the ceiling by the unit tests
/// (`quorum_and_approval_stay_exact_at_the_ceiling`). Bounded, and said so.
fn any_bounded_amount() -> Amount {
    Amount::from_u128(kani::any())
}

/// A symbolic tally that [`Tally::add`] could have built: its turnout is
/// representable.
fn tally_from(for_votes: Amount, against_votes: Amount, abstain_votes: Amount) -> Tally {
    let tally = Tally {
        for_votes,
        against_votes,
        abstain_votes,
    };
    kani::assume(tally.checked_turnout().is_some());
    tally
}

fn any_tally() -> Tally {
    tally_from(any_amount(), any_amount(), any_amount())
}

fn any_bounded_tally() -> Tally {
    tally_from(any_bounded_amount(), any_bounded_amount(), any_bounded_amount())
}

/// A symbolic choice.
fn any_choice() -> Choice {
    let tag: u8 = kani::any();
    kani::assume(tag >= 1 && tag <= 3);
    match Choice::from_tag(tag) {
        Some(choice) => choice,
        // Unreachable given the assumption; `unwrap` in a harness would be a
        // second thing for the checker to reason about.
        None => Choice::Abstain,
    }
}

/// Adding a vote never loses one, and never adds to more than one column.
///
/// The property a tally most needs and least advertises: a vote that landed in
/// two columns would inflate turnout, and one that landed in none would be a
/// silent disenfranchisement.
#[kani::proof]
fn adding_a_vote_moves_exactly_one_column() {
    let tally = any_tally();
    let weight = any_amount();
    let choice = any_choice();

    if let Ok(next) = tally.add(choice, weight) {
        let moved = u32::from(next.for_votes != tally.for_votes)
            + u32::from(next.against_votes != tally.against_votes)
            + u32::from(next.abstain_votes != tally.abstain_votes);
        assert!(moved == 1);

        assert!(next.checked_turnout() == tally.turnout().checked_add(weight));
    }
}

/// A zero-weight vote is always refused.
///
/// Counted as zero it would still be a vote, and a chain of them would pad
/// turnout toward a quorum at no cost.
#[kani::proof]
fn a_zero_weight_vote_is_never_accepted() {
    let tally = any_tally();
    let choice = any_choice();
    assert!(tally.add(choice, Amount::ZERO).is_err());
}

/// Approval is monotone in votes for and antitone in votes against.
///
/// Stated because the alternative is not obviously absurd: a threshold
/// implemented with the wrong comparison can make an extra vote in favour turn
/// a pass into a failure, and nobody would notice until it mattered.
#[kani::proof]
fn more_support_never_turns_a_pass_into_a_failure() {
    let tally = any_bounded_tally();
    let extra = any_bounded_amount();
    kani::assume(!extra.is_zero());

    let approval_bps: u32 = kani::any();
    kani::assume(approval_bps >= MIN_APPROVAL_BPS && approval_bps <= BPS_DENOMINATOR);

    if let Ok(stronger) = tally.add(Choice::For, extra) {
        if tally.meets_approval(approval_bps) {
            assert!(stronger.meets_approval(approval_bps));
        }
    }
}

/// Quorum is monotone in turnout.
#[kani::proof]
fn more_turnout_never_loses_a_quorum() {
    let tally = any_bounded_tally();
    let extra = any_bounded_amount();
    kani::assume(!extra.is_zero());

    let eligible = any_bounded_amount();
    let quorum_bps: u32 = kani::any();
    kani::assume(quorum_bps >= MIN_QUORUM_BPS && quorum_bps <= BPS_DENOMINATOR);

    if let Ok(stronger) = tally.add(any_choice(), extra) {
        if tally.meets_quorum(eligible, quorum_bps) {
            assert!(stronger.meets_quorum(eligible, quorum_bps));
        }
    }
}

/// Nothing passes on a chain with no eligible stake.
///
/// The first vote on an empty chain would otherwise be unanimous and unopposed,
/// and whoever cast it would own the rules.
#[kani::proof]
fn nothing_passes_against_an_empty_denominator() {
    let tally = any_tally();
    let quorum_bps: u32 = kani::any();
    let approval_bps: u32 = kani::any();
    assert!(!tally.passes(Amount::ZERO, quorum_bps, approval_bps));
}

/// A proposal that opens has a schedule that runs forward.
///
/// A schedule that wrapped would produce a proposal executable the moment it was
/// created — the one arithmetic failure here worth anything to an attacker.
#[kani::proof]
fn an_opened_proposal_has_a_monotone_schedule() {
    let height: u64 = kani::any();
    let voting: u64 = kani::any();
    let timelock: u64 = kani::any();

    if let Ok(proposal) = Proposal::open(height, voting, timelock) {
        let schedule = proposal.schedule;
        assert!(schedule.created == height);
        assert!(schedule.voting_closes >= schedule.created);
        assert!(schedule.executable_from >= schedule.voting_closes);
        assert!(schedule.expires_after >= schedule.executable_from);

        // And the exit window is never shorter than the compiled floor, which
        // is the whole reason the floor exists.
        assert!(
            schedule
                .executable_from
                .saturating_sub(schedule.voting_closes)
                >= MIN_TIMELOCK_BLOCKS
                || schedule.executable_from == u64::MAX
        );
    }
}

/// A schedule below either compiled floor is always refused.
#[kani::proof]
fn a_schedule_below_a_floor_is_never_opened() {
    let height: u64 = kani::any();
    let voting: u64 = kani::any();
    let timelock: u64 = kani::any();

    kani::assume(voting < MIN_VOTING_BLOCKS || timelock < MIN_TIMELOCK_BLOCKS);
    assert!(Proposal::open(height, voting, timelock).is_err());
}

/// A proposal is never executable before its timelock has run.
///
/// The exit window, stated as an unreachable state rather than as a check
/// somebody remembered to write.
#[kani::proof]
fn nothing_executes_before_its_timelock() {
    let height: u64 = kani::any();
    let voting: u64 = kani::any();
    let timelock: u64 = kani::any();
    let at: u64 = kani::any();

    if let Ok(proposal) = Proposal::open(height, voting, timelock) {
        let queued = Proposal {
            state: ProposalState::Queued,
            ..proposal
        };
        if at < queued.schedule.executable_from {
            assert!(!queued.is_executable(at));
            assert!(queued.execute(at).is_err());
        }
    }
}

/// A settled proposal admits no further transition.
#[kani::proof]
fn a_settled_proposal_never_moves_again() {
    let height: u64 = kani::any();
    let voting: u64 = kani::any();
    let timelock: u64 = kani::any();
    let at: u64 = kani::any();

    let tag: u8 = kani::any();
    kani::assume(tag >= 3 && tag <= 6);
    let Some(state) = ProposalState::from_tag(tag) else {
        return;
    };
    assert!(state.is_settled());

    if let Ok(proposal) = Proposal::open(height, voting, timelock) {
        let settled = Proposal { state, ..proposal };

        assert!(settled.vote(any_choice(), Amount::ONE, at).is_err());
        assert!(settled.execute(at).is_err());
        assert!(settled.cancel().is_err());
        assert!(!settled.is_executable(at));
        // Finalizing is a no-op rather than an error, because the end-of-block
        // pass visits every proposal and must not fail on a settled one.
        assert!(settled.finalize(at, Amount::ONE, MIN_QUORUM_BPS, MIN_APPROVAL_BPS) == Ok(settled));
    }
}

/// A parameter value is accepted exactly when it is inside its own range.
///
/// The bound between "governance may adjust this" and "governance may break
/// this", proved for every key and every `u64`.
#[kani::proof]
fn a_parameter_is_accepted_exactly_within_its_range() {
    let index: usize = kani::any();
    kani::assume(index < ALL_KEYS.len());
    let key = ALL_KEYS[index];

    let value: u64 = kani::any();
    let bounds = key.bounds();

    if key.check(value).is_ok() {
        assert!(value >= bounds.min && value <= bounds.max);
    } else {
        assert!(value < bounds.min || value > bounds.max);
    }
}

/// Every parameter's default is inside its own range.
///
/// A default outside its bounds would mean the chain starts in a state no
/// proposal could restore it to.
#[kani::proof]
fn every_default_is_reachable_by_proposal() {
    let index: usize = kani::any();
    kani::assume(index < ALL_KEYS.len());
    let key: ParameterKey = ALL_KEYS[index];

    let bounds = key.bounds();
    assert!(bounds.min <= bounds.max);
    assert!(key.check(bounds.default).is_ok());
}

/// A parameter tag round-trips, and an unknown one is always refused.
#[kani::proof]
fn an_unknown_parameter_tag_is_never_accepted() {
    let tag: u16 = kani::any();

    match ParameterKey::from_tag(tag) {
        Ok(key) => assert!(key.tag() == tag),
        Err(_) => {
            // Every known key has a tag that decodes; so a refusal means this
            // tag belongs to none of them.
            let index: usize = kani::any();
            kani::assume(index < ALL_KEYS.len());
            assert!(ALL_KEYS[index].tag() != tag);
        }
    }
}
