//! Counting votes.
//!
//! # Two thresholds, measured against different things
//!
//! **Quorum** is turnout against eligible stake: did enough of the chain show
//! up for the result to mean anything. **Approval** is `for` against
//! `for + against`: of those who expressed a preference, did a majority want it.
//!
//! Abstentions count toward quorum and not toward approval, which is what makes
//! abstaining a distinct act rather than a slower way of voting against. A
//! holder who thinks a question should be settled but has no view can say so.
//!
//! # Why quorum is measured against locked stake alone
//!
//! Voting weight is locked stake plus recent work credit, but the quorum
//! denominator is locked stake only. Work credit decays per address at a
//! different moment for each address, so a chain-wide total of it would drift
//! from the sum of its parts — and a quorum denominator that is quietly wrong
//! is worse than one that is conservatively simple.
//!
//! The consequence is that turnout can exceed 100% of the denominator when
//! miners vote. That only makes quorum easier to reach, never harder, and the
//! binding threshold is approval, which is unaffected.
//!
//! # Overflow
//!
//! Every weight is capped at [`MAX_WEIGHT`] and every accumulation is checked.
//! A tally that wrapped would not merely be wrong, it would be wrong in the
//! direction of whoever arranged the wrap.

use maya_amount::Amount;

use crate::error::{GovernanceError, Result};
use crate::limits::BPS_DENOMINATOR;

/// Largest weight one vote may carry.
///
/// 2²⁵⁶ − 1, the whole supply ceiling (ADR-046) — no address can hold more,
/// and work credit is saturated to it. Every product below is taken through
/// the 512-bit [`maya_amount::Wide`], so the checked arithmetic is a statement
/// rather than a hope.
pub const MAX_WEIGHT: Amount = Amount::MAX;

/// How an address voted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Choice {
    /// In favour.
    For,
    /// Against.
    Against,
    /// Present, with no preference.
    ///
    /// Counts toward quorum and not toward approval, so it is a way of saying
    /// the question should be settled without saying how.
    Abstain,
}

impl Choice {
    /// Stable wire tag. Never renumber — it is consensus.
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::For => 1,
            Self::Against => 2,
            Self::Abstain => 3,
        }
    }

    /// Decodes a wire tag.
    #[must_use]
    pub const fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            1 => Some(Self::For),
            2 => Some(Self::Against),
            3 => Some(Self::Abstain),
            _ => None,
        }
    }

    /// Short label for logs and explorers.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::For => "for",
            Self::Against => "against",
            Self::Abstain => "abstain",
        }
    }
}

/// Accumulated vote weight.
///
/// Invariant, kept by [`Tally::add`]: the three fields sum to at most
/// [`Amount::MAX`], so [`Tally::turnout`] and [`Tally::decisive`] are exact.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    /// Weight in favour.
    pub for_votes: Amount,
    /// Weight against.
    pub against_votes: Amount,
    /// Weight present without a preference.
    pub abstain_votes: Amount,
}

impl Tally {
    /// An empty tally.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            for_votes: Amount::ZERO,
            against_votes: Amount::ZERO,
            abstain_votes: Amount::ZERO,
        }
    }

    /// Total weight that voted at all, or `None` if the fields sum past
    /// [`Amount::MAX`] — which [`Tally::add`] never produces.
    #[must_use]
    pub const fn checked_turnout(&self) -> Option<Amount> {
        match self.for_votes.checked_add(self.against_votes) {
            Some(decisive) => decisive.checked_add(self.abstain_votes),
            None => None,
        }
    }

    /// Total weight that voted at all.
    ///
    /// Exact for every tally [`Tally::add`] builds; saturates only for one
    /// assembled by hand past the ceiling, which then reads as full turnout
    /// and is still refused approval unless its decisive votes carry it.
    #[must_use]
    pub const fn turnout(&self) -> Amount {
        match self.checked_turnout() {
            Some(turnout) => turnout,
            None => Amount::MAX,
        }
    }

    /// Weight that expressed a preference.
    #[must_use]
    pub const fn decisive(&self) -> Amount {
        self.for_votes.saturating_add(self.against_votes)
    }

    /// Adds a vote, returning the resulting tally.
    ///
    /// Returns a new value rather than mutating, so a caller that computes a
    /// tally and then rejects the vote — because the voter had already voted,
    /// or their lock was too short — cannot leave a half-counted one behind.
    ///
    /// # Errors
    ///
    /// - [`GovernanceError::NoVotingPower`] for a zero weight. Refused rather
    ///   than counted, so a zero-weight vote cannot pad turnout toward a
    ///   quorum.
    /// - [`GovernanceError::TallyOverflow`] if the accumulated turnout would
    ///   pass [`MAX_WEIGHT`].
    pub const fn add(&self, choice: Choice, weight: Amount) -> Result<Self> {
        if weight.is_zero() {
            return Err(GovernanceError::NoVotingPower);
        }
        let Some(turnout) = self.checked_turnout() else {
            return Err(GovernanceError::TallyOverflow);
        };
        // The whole turnout, not only the slot, must stay representable: that
        // is what keeps `turnout` and `decisive` exact.
        if turnout.checked_add(weight).is_none() {
            return Err(GovernanceError::TallyOverflow);
        }

        let mut next = *self;
        let slot = match choice {
            Choice::For => &mut next.for_votes,
            Choice::Against => &mut next.against_votes,
            Choice::Abstain => &mut next.abstain_votes,
        };
        match slot.checked_add(weight) {
            Some(updated) => *slot = updated,
            None => return Err(GovernanceError::TallyOverflow),
        }
        Ok(next)
    }

    /// Whether turnout reaches `quorum_bps` of `eligible`.
    ///
    /// Cross-multiplied rather than divided, so the comparison is exact. A
    /// division would floor both sides and report a quorum missed by a rounding
    /// artefact. Both products are 512-bit, so neither can overflow.
    #[must_use]
    pub fn meets_quorum(&self, eligible: Amount, quorum_bps: u32) -> bool {
        // Nothing eligible means nothing to reach a fraction of. Passing a
        // proposal on an empty chain would make the very first vote unanimous
        // and unopposed, so this reads as "not met".
        if eligible.is_zero() {
            return false;
        }
        let Some(turnout) = self.checked_turnout() else {
            // Unreachable for a tally `add` built. Reported as "not met"
            // rather than assumed away: a quorum check that failed open is a
            // quorum check that is not one.
            return false;
        };
        turnout.mul_wide(bps(BPS_DENOMINATOR)) >= eligible.mul_wide(bps(quorum_bps))
    }

    /// Whether the share in favour reaches `approval_bps` of decisive votes.
    #[must_use]
    pub fn meets_approval(&self, approval_bps: u32) -> bool {
        let Some(decisive) = self.for_votes.checked_add(self.against_votes) else {
            return false;
        };
        // Nobody expressed a preference. Abstentions can carry a quorum; they
        // cannot carry a decision.
        if decisive.is_zero() {
            return false;
        }
        self.for_votes.mul_wide(bps(BPS_DENOMINATOR)) >= decisive.mul_wide(bps(approval_bps))
    }

    /// Whether the proposal passes on both thresholds.
    #[must_use]
    pub fn passes(&self, eligible: Amount, quorum_bps: u32, approval_bps: u32) -> bool {
        self.meets_quorum(eligible, quorum_bps) && self.meets_approval(approval_bps)
    }
}

/// A basis-point figure as an [`Amount`], for cross-multiplication.
const fn bps(value: u32) -> Amount {
    Amount::from_u64(value as u64)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn a(value: u64) -> Amount {
        Amount::from_u64(value)
    }

    #[test]
    fn a_zero_weight_vote_is_refused_rather_than_counted() {
        // Counted as zero, it would still be a vote, and a chain of them would
        // pad turnout toward a quorum at no cost.
        assert_eq!(
            Tally::empty().add(Choice::For, a(0)),
            Err(GovernanceError::NoVotingPower)
        );
    }

    #[test]
    fn a_turnout_past_the_ceiling_is_refused() {
        // One vote may carry the whole ceiling; a second on any side would
        // make turnout unrepresentable, so it is refused, not wrapped.
        let full = Tally::empty().add(Choice::For, MAX_WEIGHT).expect("max");
        assert_eq!(
            full.add(Choice::Against, Amount::ONE),
            Err(GovernanceError::TallyOverflow)
        );
        assert_eq!(full.turnout(), MAX_WEIGHT);
    }

    #[test]
    fn quorum_and_approval_stay_exact_at_the_ceiling() {
        // 512-bit cross-multiplication: weights near 2^256 compare exactly.
        let half = Amount::MAX.shr(1);
        let tally = Tally::empty()
            .add(Choice::For, half)
            .expect("for")
            .add(Choice::Against, half)
            .expect("against");
        assert!(tally.meets_approval(5_000));
        assert!(!tally.meets_approval(5_001));
        assert!(tally.meets_quorum(Amount::MAX, 9_999));
    }

    #[test]
    fn abstentions_count_toward_quorum_and_not_toward_approval() {
        // The property that makes abstaining a distinct act rather than a
        // slower way of voting against.
        let tally = Tally::empty()
            .add(Choice::Abstain, a(900))
            .expect("abstain")
            .add(Choice::For, a(60))
            .expect("for")
            .add(Choice::Against, a(40))
            .expect("against");

        assert_eq!(tally.turnout(), a(1_000));
        assert_eq!(tally.decisive(), a(100));

        // All 1000 count for quorum.
        assert!(tally.meets_quorum(a(1_000), BPS_DENOMINATOR));
        // Only the 100 decisive votes decide the outcome: 60 of 100 is 60%.
        assert!(tally.meets_approval(6_000));
        assert!(!tally.meets_approval(6_001));
    }

    #[test]
    fn a_proposal_with_no_decisive_votes_does_not_pass() {
        // Abstentions can carry a quorum. They cannot carry a decision.
        let tally = Tally::empty().add(Choice::Abstain, a(1_000)).expect("abstain");
        assert!(tally.meets_quorum(a(1_000), BPS_DENOMINATOR));
        assert!(!tally.meets_approval(5_001));
        assert!(!tally.passes(a(1_000), BPS_DENOMINATOR, 5_001));
    }

    #[test]
    fn a_tie_does_not_pass_a_strict_majority_threshold() {
        let tally = Tally::empty()
            .add(Choice::For, a(500))
            .expect("for")
            .add(Choice::Against, a(500))
            .expect("against");

        // 5000 bps is exactly half, which `limits::MIN_APPROVAL_BPS` forbids
        // precisely so that this case cannot arise in practice.
        assert!(tally.meets_approval(5_000));
        assert!(!tally.meets_approval(5_001));
    }

    #[test]
    fn nothing_passes_on_an_empty_chain() {
        // With no eligible stake, the first vote would be unanimous and
        // unopposed, and whoever cast it would own the rules.
        let tally = Tally::empty().add(Choice::For, a(1)).expect("for");
        assert!(!tally.meets_quorum(a(0), 1));
        assert!(!tally.passes(a(0), 1_000, 5_001));
    }

    #[test]
    fn the_quorum_comparison_is_exact_rather_than_a_ratio_of_floors() {
        // 1 of 3 is 3333.33 bps. A threshold of exactly 3333 is met; 3334 is
        // not. Dividing first would round both sides and get one of these
        // wrong.
        let tally = Tally::empty().add(Choice::For, a(1)).expect("for");
        assert!(tally.meets_quorum(a(3), 3_333));
        assert!(!tally.meets_quorum(a(3), 3_334));
    }

    #[test]
    fn turnout_above_the_denominator_is_permitted() {
        // Work credit is not in the quorum denominator, so miners voting can
        // push turnout past 100% of locked stake. That only makes quorum
        // easier, never harder, and approval is unaffected.
        let tally = Tally::empty().add(Choice::For, a(5_000)).expect("for");
        assert!(tally.meets_quorum(a(1_000), BPS_DENOMINATOR));
        assert!(tally.meets_approval(BPS_DENOMINATOR));
    }

    #[test]
    fn adding_a_vote_does_not_disturb_the_tally_it_was_given() {
        let before = Tally::empty().add(Choice::For, a(10)).expect("for");
        let after = before.add(Choice::Against, a(5)).expect("against");

        assert_eq!(before.against_votes, Amount::ZERO);
        assert_eq!(after.against_votes, a(5));
        assert_eq!(after.for_votes, a(10));
    }
}
