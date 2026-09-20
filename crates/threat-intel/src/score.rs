//! The score: integer, additive per distinct offence, halved every
//! [`HALF_LIFE_BLOCKS`].
//!
//! # One offence confirms
//!
//! [`weight`] of either offence equals [`CONFIRM_SCORE`]. Evidence is verified
//! before it is scored, so a single conviction is as certain as a hundred;
//! requiring several would only let an attacker send one bad frame per key and
//! never be recorded.
//!
//! # Duration comes from the score, not a table
//!
//! A quarantine lasts while the decayed score is at least [`CONFIRM_SCORE`].
//! One offence: one half-life after the last. Two: two half-lives. Four: three.
//! [`active_spans`] computes that closed form, and `proofs` checks it agrees
//! with running the decay for every score and height.

use crate::evidence::OffenceKind;

/// Decayed score at which an author is quarantined.
pub const CONFIRM_SCORE: u64 = 100;

/// Blocks for a score to halve.
///
/// Heights, never timestamps: a timestamp is whatever the miner wrote
/// (invariant 9).
pub const HALF_LIFE_BLOCKS: u64 = 720;

/// Most extra half-lives a quarantine can earn by repetition.
pub const MAX_DOUBLINGS: u32 = 10;

/// Score cap: the largest score whose quarantine lasts `MAX_DOUBLINGS + 1`
/// half-lives. Past it, more offences change nothing, so an author cannot push
/// its own record towards an overflow.
pub const MAX_SCORE: u64 = (CONFIRM_SCORE << (MAX_DOUBLINGS + 1)) - 1;

/// What one verified offence adds.
#[must_use]
pub const fn weight(kind: OffenceKind) -> u64 {
    match kind {
        OffenceKind::InvalidSignature | OffenceKind::TxRootMismatch => CONFIRM_SCORE,
    }
}

/// `score` as it stands at height `to`, last updated at `from`.
///
/// A height at or before `from` leaves it unchanged: execution only moves
/// forward, and a revert restores the stored record rather than running this
/// backwards.
#[must_use]
pub const fn decay(score: u64, from: u64, to: u64) -> u64 {
    if to <= from {
        return score;
    }
    let halvings = (to - from) / HALF_LIFE_BLOCKS;
    if halvings >= u64::BITS as u64 {
        0
    } else {
        score >> halvings
    }
}

/// How many half-lives after its last offence a score keeps an author
/// quarantined: `floor(log2(score / CONFIRM_SCORE)) + 1`, or `0` below the
/// threshold.
#[must_use]
pub const fn active_spans(score: u64) -> u64 {
    let ratio = score / CONFIRM_SCORE;
    if ratio == 0 {
        0
    } else {
        (u64::BITS - 1 - ratio.leading_zeros()) as u64 + 1
    }
}

/// Whether a score last updated at `last_height` quarantines at `height`.
#[must_use]
pub const fn is_active(score: u64, last_height: u64, height: u64) -> bool {
    let spans = active_spans(score);
    // `spans` is at most 58 for any `u64`, so this product cannot overflow.
    spans != 0 && height.saturating_sub(last_height) < spans * HALF_LIFE_BLOCKS
}

/// First height at which the quarantine has lifted, or `None` if it is not in
/// force at all. Saturates at `u64::MAX`, a height no chain reaches.
#[must_use]
pub const fn until_height(score: u64, last_height: u64) -> Option<u64> {
    let spans = active_spans(score);
    if spans == 0 {
        None
    } else {
        Some(last_height.saturating_add(spans * HALF_LIFE_BLOCKS))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn one_offence_lasts_one_half_life() {
        let score = weight(OffenceKind::InvalidSignature);
        assert!(is_active(score, 10, 10));
        assert!(is_active(score, 10, 10 + HALF_LIFE_BLOCKS - 1));
        assert!(!is_active(score, 10, 10 + HALF_LIFE_BLOCKS));
        assert_eq!(until_height(score, 10), Some(10 + HALF_LIFE_BLOCKS));
    }

    #[test]
    fn repetition_adds_half_lives_logarithmically() {
        assert_eq!(active_spans(CONFIRM_SCORE - 1), 0);
        assert_eq!(active_spans(CONFIRM_SCORE), 1);
        assert_eq!(active_spans(2 * CONFIRM_SCORE), 2);
        assert_eq!(active_spans(3 * CONFIRM_SCORE), 2);
        assert_eq!(active_spans(4 * CONFIRM_SCORE), 3);
        assert_eq!(active_spans(MAX_SCORE), u64::from(MAX_DOUBLINGS) + 1);
        assert_eq!(active_spans(MAX_SCORE + 1), u64::from(MAX_DOUBLINGS) + 2);
    }

    #[test]
    fn the_closed_form_agrees_with_running_the_decay() {
        for score in [0, 99, 100, 150, 200, 399, 400, 1_000, MAX_SCORE] {
            for elapsed in (0..12 * HALF_LIFE_BLOCKS).step_by(97) {
                assert_eq!(
                    decay(score, 5, 5 + elapsed) >= CONFIRM_SCORE,
                    is_active(score, 5, 5 + elapsed),
                    "score {score} after {elapsed} blocks"
                );
            }
        }
    }

    #[test]
    fn decay_never_runs_backwards_and_empties_eventually() {
        assert_eq!(decay(400, 100, 50), 400);
        assert_eq!(decay(400, 0, HALF_LIFE_BLOCKS), 200);
        assert_eq!(decay(u64::MAX, 0, u64::MAX), 0);
    }
}
