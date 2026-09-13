//! Ledger value arithmetic for `custom-l1-node`, isolated so it can be proved.
//!
//! # Why this is a crate and not a module
//!
//! Every operation here is three lines of `checked_add` or `checked_sub`. That
//! is not what justifies the crate boundary — verifiability is.
//!
//! The [Kani Rust Verifier] compiles a crate *and its whole dependency graph*
//! into a CBMC goto-binary. The node depends on `rocksdb` (C++ through
//! `librocksdb-sys`), `libp2p`, `fips204`, and the `ark-*` SNARK stack. Kani
//! has no semantics for the C++ and no appetite for the rest, so
//! `cargo kani -p custom-l1-node` is not a slow proposition, it is not a
//! possible one.
//!
//! A dependency-free leaf crate is. The node calls into this crate rather than
//! inlining the arithmetic, so what the proofs in `proofs` (compiled only under Kani) establish is a
//! property of the code that actually executes when a block is applied — not of
//! a restatement of it that could drift.
//!
//! # What is proved, and what is not
//!
//! Every scalar function here has an *unbounded* proof: the harness quantifies
//! over all `u64` inputs, and CBMC discharges it without a loop bound.
//!
//! [`total_outputs`] is the exception. It folds over a sequence, so its harness
//! carries a `#[kani::unwind]` bound and an accompanying `kani::assume` on the
//! length. That proof is bounded — it holds for sequences up to the bound, not
//! for all sequences. It is stated that way in `proofs` (compiled only under Kani) rather than left for
//! a reader to infer.
//!
//! # What these functions deliberately do not do
//!
//! They return [`Option`], not the node's `NodeError`. Keeping `custom-l1-node`
//! out of this crate's dependencies is the whole point, and an error type is
//! not arithmetic: the caller owns the mapping from "this would have wrapped"
//! to whichever `NodeError` variant its context calls for.
//!
//! [Kani Rust Verifier]: https://model-checking.github.io/kani/

// `no_std` in every build but the test harness, which needs `std` to run.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

#[cfg(kani)]
pub mod proofs;

/// Adds `amount` into `balance`.
///
/// Returns `None` exactly when the sum would exceed [`u64::MAX`]. Total supply
/// is a `u64`, so a credit that would wrap is a credit that would mint value
/// out of nothing — the caller must reject the transaction, never truncate.
#[must_use]
pub const fn credit(balance: u64, amount: u64) -> Option<u64> {
    balance.checked_add(amount)
}

/// Removes `amount` from `balance`.
///
/// Returns `None` exactly when `amount > balance`. Callers generally check
/// sufficiency first and could use plain subtraction; they should not. A
/// wrapping debit turns an overdraft into a balance near [`u64::MAX`], which is
/// the single worst failure mode a ledger has, and it costs nothing to make it
/// unrepresentable rather than merely unreachable.
#[must_use]
pub const fn debit(balance: u64, amount: u64) -> Option<u64> {
    balance.checked_sub(amount)
}

/// Advances an account's replay counter by one.
///
/// Returns `None` at [`u64::MAX`]. Unreachable in practice — it would take
/// 2^64 transactions from one account — but a nonce that wrapped would make
/// every transaction that account ever sent replayable, so it is checked rather
/// than assumed.
#[must_use]
pub const fn advance_nonce(nonce: u64) -> Option<u64> {
    nonce.checked_add(1)
}

/// Sums the amounts a transaction's outputs assign.
///
/// Returns `None` if the running total would wrap at any point, *not* only if
/// the final total would. The distinction matters: `[u64::MAX, 1, u64::MAX]`
/// has no representable sum, and an implementation that wrapped through the
/// middle and happened to land somewhere small would report a total the
/// transaction does not actually move.
#[must_use]
pub fn total_outputs(amounts: impl IntoIterator<Item = u64>) -> Option<u64> {
    let mut total: u64 = 0;
    for amount in amounts {
        total = total.checked_add(amount)?;
    }
    Some(total)
}

/// Splits `total` across holders in proportion to `weights`, exactly.
///
/// ## Why this is here and not in the RWA crate
///
/// Because every function in this crate decides how much value moves, and this
/// one decides it ten thousand times in a block. It is dependency-free for the
/// same reason [`settle_pool`] is: Kani compiles a crate with its whole
/// dependency graph, and `Sigma payouts == total` is exactly the kind of claim a
/// model checker should be settling rather than a test sampling.
///
/// ## The property
///
/// **`payouts.iter().sum() == total`**, always. Not approximately, not up to
/// dust. Maya2C's invariant guard refuses any block whose value deltas do not
/// balance, so a distribution that lost a base unit to rounding would not be a
/// small unfairness — it would be a block nobody can mine.
///
/// ## Largest remainder, and why the tie-break is load-bearing
///
/// Every holder first gets `floor(total * weight / total_weight)`. That leaves
/// `k` base units unassigned, `k < holders`. Those go to the `k` holders with
/// the largest fractional remainders — Hamilton's method, which keeps every
/// holder within one base unit of their exact share.
///
/// Two holders can have the **same** remainder. If the order between them were
/// unspecified, two validators would hand the spare unit to different accounts,
/// produce different state roots, and the chain would split over one base unit.
/// So the ranking is `(remainder, index)` and the index breaks every tie: total,
/// deterministic, and identical on every machine.
///
/// ## No allocation
///
/// The caller supplies `payouts` and `order`. This runs inside block execution
/// with ten thousand holders, which is exactly the "critical consensus loop" the
/// execution directives say not to allocate in — and `no_std` here means there
/// is no allocator to reach for even by accident.
///
/// `slice::sort_unstable_by` is used rather than `sort_by`: the stable sort
/// allocates, and stability buys nothing once the index is in the key.
///
/// # Errors
///
/// Returns `None` when `payouts` or `order` is not the same length as
/// `weights`, when the weights sum to zero — there is nothing to be in
/// proportion to — or when they overflow a `u64`.
#[must_use]
pub fn distribute(
    total: u64,
    weights: &[u64],
    payouts: &mut [u64],
    order: &mut [u32],
) -> Option<()> {
    if payouts.len() != weights.len() || order.len() != weights.len() {
        return None;
    }
    if u32::try_from(weights.len()).is_err() {
        return None;
    }
    if weights.is_empty() {
        // Nothing to distribute to. A caller with a zero total and no holders
        // is consistent; one with a non-zero total is trying to pay nobody, and
        // the value would vanish.
        return (total == 0).then_some(());
    }

    let total_weight = total_outputs(weights.iter().copied())?;
    if total_weight == 0 {
        return None;
    }

    // u128 throughout: `total * weight` reaches 2^128 for two u64 operands, and
    // a product that wrapped would produce a share nobody is owed.
    let total_wide = u128::from(total);
    let weight_wide = u128::from(total_weight);

    let mut assigned: u128 = 0;
    for (index, weight) in weights.iter().enumerate() {
        let exact = total_wide * u128::from(*weight);
        let floor = exact / weight_wide;
        payouts[index] = floor as u64;
        assigned += floor;
        order[index] = index as u32;
    }

    // What the flooring left behind. Strictly less than the holder count, so it
    // always fits.
    let mut spare = (total_wide - assigned) as usize;
    if spare == 0 {
        return Some(());
    }

    // Rank by remainder, then by index. Descending on the remainder so the
    // largest come first; ascending on the index so a tie resolves the same way
    // on every node, which is the whole reason the index is in the key.
    let remainder = |index: u32| -> u128 {
        let weight = u128::from(weights[index as usize]);
        (total_wide * weight) % weight_wide
    };
    order.sort_unstable_by(|left, right| {
        remainder(*right)
            .cmp(&remainder(*left))
            .then_with(|| left.cmp(right))
    });

    for index in order.iter() {
        if spare == 0 {
            break;
        }
        // A holder cannot be paid past u64 by one extra base unit unless their
        // floor was already u64::MAX, which needs a total that large.
        payouts[*index as usize] = payouts[*index as usize].checked_add(1)?;
        spare -= 1;
    }
    Some(())
}

/// Sums the two sides of a channel closure.
///
/// Distinct from [`credit`] despite the identical body, because the check means
/// something different: this is the total a closure claims to distribute, and
/// it is compared against what the channel actually escrowed. Naming it
/// `credit` at the call site would read as though a balance were being paid
/// into, which is not what happens.
#[must_use]
pub const fn combined_balance(balance_a: u64, balance_b: u64) -> Option<u64> {
    balance_a.checked_add(balance_b)
}

/// Why [`settle_pool`] could not produce a new pool balance.
///
/// Two cases rather than one because the callers must distinguish them: an
/// overflow is a malformed transaction, while an underflow means value left the
/// shielded pool that the circuit should have proved was in it. The second is
/// supposed to be unreachable, and reporting it as "overflow" would erase the
/// one signal that a soundness break had occurred.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettleError {
    /// `public_out + fee`, or `held + public_in`, would exceed [`u64::MAX`].
    Overflow,
    /// More value left the pool than it held.
    Underflow {
        /// Pool balance after crediting `public_in`.
        held: u64,
        /// Total leaving the pool: `public_out + fee`.
        withdrawn: u64,
    },
}

/// Moves value across the shielded pool boundary.
///
/// `public_in` enters the pool; `public_out` and `fee` leave it — the fee is
/// paid out of shielded value to a transparent sink, so it is a withdrawal like
/// any other. Returns the pool's new balance.
///
/// # Errors
///
/// [`SettleError::Overflow`] if either sum would wrap, and
/// [`SettleError::Underflow`] if the withdrawal exceeds what the pool holds.
pub const fn settle_pool(
    held: u64,
    public_in: u64,
    public_out: u64,
    fee: u64,
) -> Result<u64, SettleError> {
    // `match` rather than `let ... else` throughout: this is a `const fn`, and
    // the narrower construct is the one guaranteed to stay const-evaluable.
    let withdrawn = match public_out.checked_add(fee) {
        Some(withdrawn) => withdrawn,
        None => return Err(SettleError::Overflow),
    };
    let credited = match held.checked_add(public_in) {
        Some(credited) => credited,
        None => return Err(SettleError::Overflow),
    };
    match credited.checked_sub(withdrawn) {
        Some(remaining) => Ok(remaining),
        None => Err(SettleError::Underflow {
            held: credited,
            withdrawn,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credit_reports_the_ceiling() {
        assert_eq!(credit(1, 2), Some(3));
        assert_eq!(credit(u64::MAX, 0), Some(u64::MAX));
        assert_eq!(credit(u64::MAX, 1), None);
    }

    #[test]
    fn debit_reports_an_overdraft() {
        assert_eq!(debit(3, 2), Some(1));
        assert_eq!(debit(3, 3), Some(0));
        assert_eq!(debit(3, 4), None);
    }

    #[test]
    fn advance_nonce_stops_at_the_ceiling() {
        assert_eq!(advance_nonce(0), Some(1));
        assert_eq!(advance_nonce(u64::MAX), None);
    }

    #[test]
    fn total_outputs_of_nothing_is_zero() {
        assert_eq!(total_outputs([]), Some(0));
    }

    #[test]
    fn total_outputs_rejects_a_wrap_through_the_middle() {
        // The sum of these three is not representable. An implementation that
        // wrapped on the second addition would report `Some(u64::MAX)` here.
        assert_eq!(total_outputs([u64::MAX, 1, u64::MAX]), None);
    }

    #[test]
    fn total_outputs_sums_the_ordinary_case() {
        assert_eq!(total_outputs([1, 2, 3]), Some(6));
    }

    #[test]
    fn settle_pool_moves_value_both_ways() {
        assert_eq!(settle_pool(100, 50, 30, 5), Ok(115));
        assert_eq!(settle_pool(0, 100, 0, 0), Ok(100));
    }

    #[test]
    fn settle_pool_separates_underflow_from_overflow() {
        assert_eq!(
            settle_pool(10, 0, 20, 0),
            Err(SettleError::Underflow {
                held: 10,
                withdrawn: 20
            })
        );
        assert_eq!(settle_pool(0, 0, u64::MAX, 1), Err(SettleError::Overflow));
        assert_eq!(settle_pool(u64::MAX, 1, 0, 0), Err(SettleError::Overflow));
    }

    #[test]
    fn settle_pool_reports_the_credited_balance_on_underflow() {
        // `held` in the error is the balance *after* `public_in` landed, which
        // is the number that makes the failure legible: it is what the pool
        // actually had when the withdrawal was attempted.
        assert_eq!(
            settle_pool(10, 5, 20, 0),
            Err(SettleError::Underflow {
                held: 15,
                withdrawn: 20
            })
        );
    }
}

#[cfg(test)]
mod distribution_tests {
    use super::*;

    /// Runs a distribution and returns the payouts.
    fn split(total: u64, weights: &[u64]) -> Vec<u64> {
        let mut payouts = vec![0u64; weights.len()];
        let mut order = vec![0u32; weights.len()];
        distribute(total, weights, &mut payouts, &mut order).expect("distribute");
        payouts
    }

    #[test]
    fn the_payouts_always_sum_to_the_total() {
        // The property the invariant guard will enforce anyway: a distribution
        // that lost a base unit is not a small unfairness, it is a block nobody
        // can mine.
        for total in [0u64, 1, 7, 100, 999_983, u64::MAX / 4] {
            for weights in [
                vec![1u64],
                vec![1, 1, 1],
                vec![1, 2, 3, 4, 5, 6, 7],
                vec![1_000_000, 1, 1],
                vec![3; 97],
            ] {
                let payouts = split(total, &weights);
                assert_eq!(
                    payouts.iter().sum::<u64>(),
                    total,
                    "total {total} across {weights:?}"
                );
            }
        }
    }

    #[test]
    fn an_exact_division_leaves_no_remainder_to_place() {
        assert_eq!(split(100, &[1, 1, 1, 1]), vec![25, 25, 25, 25]);
        assert_eq!(split(90, &[1, 2]), vec![30, 60]);
    }

    #[test]
    fn the_spare_units_go_to_the_largest_remainders() {
        // 10 across weights 1,1,1: each is 3.333, so three floors of 3 and one
        // spare unit. Every remainder is equal, so the tie-break decides — and
        // it must decide the same way every time.
        assert_eq!(split(10, &[1, 1, 1]), vec![4, 3, 3]);

        // 7 across 1,1,1,1: floors of 1 each, three spare. Again all tied.
        assert_eq!(split(7, &[1, 1, 1, 1]), vec![2, 2, 2, 1]);

        // 10 across 1,1,4: floors are 1, 1, 6 — eight of ten placed, so *two*
        // spares, and every remainder is 4, so the two lowest indices take
        // them. Counting the spares wrong is easy and the sum catches it.
        assert_eq!(split(10, &[1, 1, 4]), vec![2, 2, 6]);
    }

    #[test]
    fn every_holder_lands_within_one_base_unit_of_their_exact_share() {
        // What largest-remainder buys over rounding down and keeping the dust.
        let weights: Vec<u64> = (1..=200u64).collect();
        let total = 1_000_003u64;
        let payouts = split(total, &weights);
        let total_weight: u128 = weights.iter().map(|w| u128::from(*w)).sum();

        for (payout, weight) in payouts.iter().zip(&weights) {
            let exact = u128::from(total) * u128::from(*weight) / total_weight;
            let paid = u128::from(*payout);
            assert!(
                paid == exact || paid == exact + 1,
                "paid {paid} where the exact share is {exact}"
            );
        }
    }

    #[test]
    fn ties_resolve_by_index_on_every_run() {
        // The fork this prevents: two validators handing the spare unit to
        // different accounts and producing different state roots. Every weight
        // here is identical, so every remainder is identical, and only the
        // index can decide.
        let weights = vec![5u64; 64];
        let first = split(1_000, &weights);
        for _ in 0..20 {
            assert_eq!(split(1_000, &weights), first);
        }
        // The spare units went to the lowest indices, in order.
        let spare = 1_000 % 64;
        for (index, payout) in first.iter().enumerate() {
            let expected = 1_000 / 64 + u64::from(index < spare as usize);
            assert_eq!(*payout, expected, "holder {index}");
        }
    }

    #[test]
    fn a_holder_with_no_weight_is_paid_nothing() {
        assert_eq!(split(100, &[0, 1, 0, 1]), vec![0, 50, 0, 50]);
    }

    #[test]
    fn ten_thousand_holders_still_sum_exactly() {
        // The block this subsystem exists for. Not a performance claim — a
        // correctness one: the sum has to be exact at the scale it will run at,
        // not only at the scale a hand-written case reaches.
        let weights: Vec<u64> = (0..10_000u64).map(|index| index % 97 + 1).collect();
        let total = 123_456_789u64;
        let payouts = split(total, &weights);
        assert_eq!(payouts.iter().sum::<u64>(), total);
        assert_eq!(payouts.len(), 10_000);
    }

    #[test]
    fn a_distribution_with_no_holders_moves_nothing_and_refuses_a_total() {
        let mut payouts: Vec<u64> = Vec::new();
        let mut order: Vec<u32> = Vec::new();
        assert!(distribute(0, &[], &mut payouts, &mut order).is_some());
        // Paying a non-zero total to nobody would make the value vanish.
        assert!(distribute(1, &[], &mut payouts, &mut order).is_none());
    }

    #[test]
    fn weights_that_sum_to_zero_are_refused() {
        // There is nothing to be in proportion to, and dividing by the total
        // weight would be a division by zero.
        let mut payouts = vec![0u64; 3];
        let mut order = vec![0u32; 3];
        assert!(distribute(10, &[0, 0, 0], &mut payouts, &mut order).is_none());
    }

    #[test]
    fn mismatched_buffers_are_refused_rather_than_truncating() {
        let mut short = vec![0u64; 2];
        let mut order = vec![0u32; 3];
        assert!(distribute(10, &[1, 1, 1], &mut short, &mut order).is_none());

        let mut payouts = vec![0u64; 3];
        let mut short_order = vec![0u32; 2];
        assert!(distribute(10, &[1, 1, 1], &mut payouts, &mut short_order).is_none());
    }

    #[test]
    fn weights_that_overflow_a_u64_are_refused() {
        let mut payouts = vec![0u64; 2];
        let mut order = vec![0u32; 2];
        assert!(distribute(10, &[u64::MAX, 1], &mut payouts, &mut order).is_none());
    }

    #[test]
    fn a_total_at_the_top_of_the_range_does_not_wrap() {
        // `total * weight` reaches 2^128 for two u64 operands, and a product
        // that wrapped would produce a share nobody is owed.
        let weights = vec![u64::MAX / 4, u64::MAX / 4];
        let payouts = split(u64::MAX, &weights);
        assert_eq!(payouts.iter().sum::<u64>(), u64::MAX);
    }
}
