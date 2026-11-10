//! Threshold monotonicity at full 256-bit width, by deterministic sweep.
//!
//! The Kani harnesses `more_support_never_turns_a_pass_into_a_failure` and
//! `more_turnout_never_loses_a_quorum` prove these two implications only for
//! weights below 2^8 (see `src/proofs.rs`). This file checks the same
//! implications over weights on every limb boundary, at the ceiling, and over
//! pseudo-random 256-bit values, where the 512-bit products have all four
//! limbs in use. It is evidence, not a proof.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_amount::Amount;
use maya_governance::limits::{BPS_DENOMINATOR, MIN_APPROVAL_BPS, MIN_QUORUM_BPS};
use maya_governance::tally::{Choice, Tally};

/// Shifts of `Amount::MAX` that land on each limb boundary, plus the ceiling.
const SHIFTS: [u32; 11] = [0, 1, 63, 64, 65, 127, 128, 129, 191, 192, 255];

/// Weights on every limb boundary, at and just below the ceiling, and a few
/// fixed 256-bit patterns.
fn edge_weights() -> Vec<Amount> {
    let mut out: Vec<Amount> = SHIFTS
        .iter()
        .flat_map(|&s| {
            let top = Amount::MAX.shr(s);
            [Some(top), top.checked_sub(Amount::from_u64(1))]
        })
        .flatten()
        .collect();
    out.push(Amount::from_u64(1));
    out.push(Amount::from_u64(u64::MAX));
    out.push(Amount::from_u128(u128::MAX));
    out.push(Amount([0, 0, 0, 1]));
    out.push(Amount([u64::MAX, u64::MAX, u64::MAX, u64::MAX >> 1]));
    out
}

/// A fixed-seed xorshift generator: the sweep is the same on every run.
struct Xorshift(u64);

impl Xorshift {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// A 256-bit value whose top limb is shifted down by a random amount, so
    /// every magnitude from 1 bit to 256 bits is drawn.
    fn amount(&mut self) -> Amount {
        let limbs = [
            self.next_u64(),
            self.next_u64(),
            self.next_u64(),
            self.next_u64(),
        ];
        let width = (self.next_u64() % 256) as u32;
        Amount(limbs).shr(256 - width.max(1))
    }
}

/// Tallies built the way the chain builds them: through `Tally::add`, so the
/// turnout is representable. Pairs that `add` refuses are skipped.
fn tallies_from(weights: &[Amount]) -> Vec<Tally> {
    let mut out = vec![Tally::empty()];
    for &w in weights {
        for choice in [Choice::For, Choice::Against, Choice::Abstain] {
            if let Ok(t) = Tally::empty().add(choice, w) {
                out.push(t);
            }
        }
    }
    for pair in weights.windows(2) {
        let built = Tally::empty()
            .add(Choice::For, pair[0])
            .and_then(|t| t.add(Choice::Against, pair[1]));
        if let Ok(t) = built {
            out.push(t);
        }
    }
    out
}

/// Thresholds the sweep tries: both ends, the ceiling, and a spread between.
fn thresholds() -> Vec<u32> {
    let mut out = vec![MIN_APPROVAL_BPS, MIN_APPROVAL_BPS + 1, BPS_DENOMINATOR];
    out.extend([6_667, 7_500, 9_999, BPS_DENOMINATOR - 1]);
    out
}

fn quorum_thresholds() -> Vec<u32> {
    let mut out = vec![MIN_QUORUM_BPS, MIN_QUORUM_BPS + 1, BPS_DENOMINATOR];
    out.extend([3_000, 5_000, 9_999]);
    out
}

#[test]
fn more_support_never_turns_a_pass_into_a_failure_at_256_bits() {
    let mut rng = Xorshift(0x9E37_79B9_7F4A_7C15);
    let mut weights = edge_weights();
    weights.extend((0..64).map(|_| rng.amount()));
    let extras: Vec<Amount> = weights.iter().copied().filter(|w| !w.is_zero()).collect();
    let thresholds = thresholds();
    let mut premises = 0;

    for tally in tallies_from(&weights) {
        for &extra in &extras {
            let Ok(stronger) = tally.add(Choice::For, extra) else {
                continue;
            };
            for &bps in &thresholds {
                if tally.meets_approval(bps) {
                    premises += 1;
                    assert!(
                        stronger.meets_approval(bps),
                        "pass became fail: {tally:?} + {extra} at {bps} bps"
                    );
                }
            }
        }
    }
    // Non-vacuity: the premise must hold for some pairs, or the sweep proves nothing.
    assert!(premises > 0);
}

#[test]
fn more_turnout_never_loses_a_quorum_at_256_bits() {
    let mut rng = Xorshift(0xD1B5_4A32_D192_ED03);
    let mut weights = edge_weights();
    weights.extend((0..64).map(|_| rng.amount()));
    let eligibles: Vec<Amount> = edge_weights()
        .into_iter()
        .filter(|w| !w.is_zero())
        .collect();
    let extras: Vec<Amount> = weights.iter().copied().filter(|w| !w.is_zero()).collect();
    let thresholds = quorum_thresholds();

    let mut premises = 0;

    for tally in tallies_from(&weights) {
        for &extra in &extras {
            let Ok(stronger) = tally.add(Choice::Abstain, extra) else {
                continue;
            };
            for &eligible in &eligibles {
                for &bps in &thresholds {
                    if tally.meets_quorum(eligible, bps) {
                        premises += 1;
                        assert!(
                            stronger.meets_quorum(eligible, bps),
                            "quorum lost: {tally:?} + {extra} of {eligible} at {bps} bps"
                        );
                    }
                }
            }
        }
    }
    assert!(premises > 0);
}
