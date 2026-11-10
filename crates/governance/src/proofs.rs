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
//! - Every harness here is **unbounded** over the inputs it quantifies,
//!   except the two threshold harnesses
//!   (`more_support_never_turns_a_pass_into_a_failure`,
//!   `more_turnout_never_loses_a_quorum`): they quantify over weights below
//!   2^8, because CBMC returns no verdict for these threshold products at 16
//!   bits or wider within the budget (ADR-046). The
//!   256-bit ceiling is covered by a unit test and a deterministic sweep, not
//!   a proof.
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

/// A symbolic weight below 2^8: a `u8` zero-extended, so the threshold products
/// are single-limb.
///
/// The threshold harnesses prove `e·a ≤ e·K` for a symbolic threshold `a` and
/// a symbolic extra weight `e`: a multiplier-vs-multiplier comparison that
/// CBMC closes in about 100 s at 8 bits. At 16 bits it gave no verdict in 10
/// minutes, at 64 bits none in 30 minutes (2026-11-10 local), and at 128 bits
/// the nightly run passed 2 hours (2026-11-09). So these two harnesses prove the threshold logic on
/// single-limb weights only. Multi-limb products (weights at and above 2^64)
/// are covered by the deterministic sweep in `tests/threshold_extremes_tests.rs`,
/// not by this proof.
fn any_bounded_amount() -> Amount {
    Amount::from_u64(u64::from(kani::any::<u8>()))
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
    tally_from(
        any_bounded_amount(),
        any_bounded_amount(),
        any_bounded_amount(),
    )
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
///
/// The four settled tags (3..=6) are taken one by one rather than as one
/// symbolic tag: the same states, all of them, but each a constant, so CBMC
/// drops `finalize`'s tally branch (a 512-bit product) that no settled state
/// reaches. With a symbolic tag the harness ran past 15 minutes (2026-11-09).
#[kani::proof]
fn a_settled_proposal_never_moves_again() {
    let height: u64 = kani::any();
    let voting: u64 = kani::any();
    let timelock: u64 = kani::any();
    let at: u64 = kani::any();

    for tag in 3u8..=6 {
        let Some(state) = ProposalState::from_tag(tag) else {
            panic!("tag {tag} names no state");
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
            assert!(
                settled.finalize(at, Amount::ONE, MIN_QUORUM_BPS, MIN_APPROVAL_BPS) == Ok(settled)
            );
        }
    }
}

/// A parameter value is accepted exactly when it is inside its own range.
///
/// The bound between "governance may adjust this" and "governance may break
/// this", proved for every key and every 256-bit value.
#[kani::proof]
fn a_parameter_is_accepted_exactly_within_its_range() {
    let index: usize = kani::any();
    kani::assume(index < ALL_KEYS.len());
    let key = ALL_KEYS[index];

    let value = any_amount();
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
