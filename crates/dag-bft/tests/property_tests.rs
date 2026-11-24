//! Property tests for committee thresholds (ADR-039, ADR-040).
//!
//! The two conditions the doc on `Committee::quorum` states, over every
//! equal-weight size and arbitrary stake weights: two quorums overlap in
//! more than the faulty weight (safety), and the weight that is not faulty
//! still reaches a quorum (liveness).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_dag_bft::Committee;
use proptest::prelude::*;

fn check(committee: &Committee, total: u128) -> Result<(), TestCaseError> {
    let (q, f) = (committee.quorum(), committee.faults());
    prop_assert!(total > 3 * f, "f too large: W={total} f={f}");
    prop_assert!(
        2 * q > total + f,
        "quorums may share no honest weight: W={total} q={q} f={f}"
    );
    prop_assert!(
        q <= total - f,
        "honest weight cannot reach quorum: W={total} q={q} f={f}"
    );
    prop_assert!(committee.validity() == f + 1);
    // The seat dimension of the dual quorum (ADR-040 amendment 2026-11-24).
    let n = u128::from(committee.size());
    let (qc, fc) = (
        u128::from(committee.count_quorum()),
        u128::from(committee.count_faults()),
    );
    prop_assert!(n > 3 * fc, "f too large by seats: n={n} f={fc}");
    prop_assert!(
        2 * qc > n + fc,
        "quorums may share no honest seat: n={n} q={qc}"
    );
    prop_assert!(
        qc <= n - fc,
        "honest seats cannot reach quorum: n={n} q={qc}"
    );
    prop_assert!(u128::from(committee.count_validity()) == fc + 1);
    // Every member is a quorum together; the floor never exceeds the seats.
    prop_assert!(committee.is_quorum(0..committee.size()));
    prop_assert!(u128::from(committee.min_quorum_size()) <= n);
    Ok(())
}

proptest! {
    #[test]
    fn equal_weight_thresholds_are_safe_and_live(size in 1u16..=512) {
        check(&Committee::new(size), u128::from(size))?;
    }

    #[test]
    fn stake_weight_thresholds_are_safe_and_live(
        weights in prop::collection::vec(1u64..=1_000_000_000, 1..64),
    ) {
        let total = weights.iter().map(|&w| u128::from(w)).sum();
        let committee = Committee::weighted(weights).expect("non-empty, non-zero weights");
        check(&committee, total)?;
    }
}
