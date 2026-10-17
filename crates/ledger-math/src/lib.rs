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
//! inlining the arithmetic, so what the proofs in `proofs` (compiled only under
//! Kani) establish is a property of the code that actually executes when a
//! block is applied — not of a restatement of it that could drift.
//!
//! # What is proved, and what is not
//!
//! Every scalar function here has an *unbounded* proof: the harness quantifies
//! over all inputs (every `u64`, or every 256-bit `Amount`), and CBMC discharges it without a loop bound.
//!
//! [`total_outputs`] is the exception. It folds over a sequence, so its harness
//! carries a `#[kani::unwind]` bound and an accompanying `kani::assume` on the
//! length. That proof is bounded — it holds for sequences up to the bound, not
//! for all sequences. It is stated that way in `proofs` (compiled only under
//! Kani) rather than left for a reader to infer.
//!
//! [`distribute`] is only partly proved: Kani covers its placement of the
//! spare, and its flooring is checked by exhaustive enumeration over a small
//! domain. The split, and why, is at the top of `proofs`.
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

pub use maya_amount::Amount;

/// Adds `amount` into `balance`.
///
/// Returns `None` exactly when the sum would exceed [`Amount::MAX`]. Total
/// supply is at most 2²⁵⁶ − 1 (ADR-046), so a credit that would wrap is a credit
/// that would mint value out of nothing — the caller must reject the
/// transaction, never truncate.
#[must_use]
pub const fn credit(balance: Amount, amount: Amount) -> Option<Amount> {
    balance.checked_add(amount)
}

/// Removes `amount` from `balance`.
///
/// Returns `None` exactly when `amount > balance`. Callers generally check
/// sufficiency first and could use plain subtraction; they should not. A
/// wrapping debit turns an overdraft into a balance near [`Amount::MAX`], which
/// is the single worst failure mode a ledger has, and it costs nothing to make
/// it unrepresentable rather than merely unreachable.
#[must_use]
pub const fn debit(balance: Amount, amount: Amount) -> Option<Amount> {
    balance.checked_sub(amount)
}

/// Advances an account's replay counter by one.
///
/// Returns `None` at [`u64::MAX`]. Unreachable in practice — it would take
/// 2^64 transactions from one account — but a nonce that wrapped would make
/// every transaction that account ever sent replayable, so it is checked rather
/// than assumed. A nonce counts transactions, not value, so it stays a `u64`.
#[must_use]
pub const fn advance_nonce(nonce: u64) -> Option<u64> {
    nonce.checked_add(1)
}

/// Sums the amounts a transaction's outputs assign.
///
/// Returns `None` if the running total would wrap at any point, *not* only if
/// the final total would. The distinction matters: `[MAX, 1, MAX]` has no
/// representable sum, and an implementation that wrapped through the middle and
/// happened to land somewhere small would report a total the transaction does
/// not actually move.
#[must_use]
pub fn total_outputs(amounts: impl IntoIterator<Item = Amount>) -> Option<Amount> {
    let mut total = Amount::ZERO;
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
/// model checker should be settling rather than a test sampling. Kani settles
/// the half that places the spare; the flooring divides by a symbolic weight,
/// which it could not finish, so that half is enumerated exhaustively over a
/// small domain instead (`every_small_distribution_is_exact_and_fair`).
///
/// ## The property
///
/// **`payouts.iter().sum() == total`**, always. Not approximately, not up to
/// dust. `Maya2C`'s invariant guard refuses any block whose value deltas do not
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
/// The caller supplies `payouts` and `order`. `order` is scratch space for the
/// ranking: its contents afterwards are unspecified, and it is left untouched
/// when there is no spare unit to place. This runs inside block execution
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
/// proportion to — or when they overflow an [`Amount`].
#[must_use]
pub fn distribute(
    total: Amount,
    weights: &[Amount],
    payouts: &mut [Amount],
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
        return total.is_zero().then_some(());
    }

    let total_weight = total_outputs(weights.iter().copied())?;
    if total_weight.is_zero() {
        return None;
    }

    let spare = floor_shares(total, weights, total_weight, payouts)?;
    if spare == 0 {
        return Some(());
    }

    for (index, slot) in (0u32..).zip(order.iter_mut()) {
        *slot = index;
    }

    // Rank by remainder, then by index. Descending on the remainder so the
    // largest come first; ascending on the index so a tie resolves the same way
    // on every node, which is the whole reason the index is in the key. The
    // remainder is `total * weight mod total_weight`, taken through the 512-bit
    // product so it is exact. The division cannot fail (a non-zero divisor and
    // a quotient at most `total`), so a failure ranks as zero rather than
    // panicking inside block execution.
    let remainder = |index: u32| -> Amount {
        total
            .mul_wide(weights[index as usize])
            .checked_div_rem(total_weight)
            .map_or(Amount::ZERO, |(_, rem)| rem)
    };
    order.sort_unstable_by(|left, right| {
        remainder(*right)
            .cmp(&remainder(*left))
            .then_with(|| left.cmp(right))
    });

    place_spare(payouts, order, spare)
}

/// Writes each holder's floored share into `payouts` and returns what the
/// flooring left behind, which is strictly less than the holder count.
///
/// The first half of [`distribute`]. `total_weight` is the sum of `weights`,
/// non-zero, and `payouts` is as long as `weights` — `distribute` checks all
/// three, so `None` here is unreachable from it; it is propagated rather than
/// unwrapped.
pub(crate) fn floor_shares(
    total: Amount,
    weights: &[Amount],
    total_weight: Amount,
    payouts: &mut [Amount],
) -> Option<usize> {
    let mut assigned = Amount::ZERO;
    for (payout, weight) in payouts.iter_mut().zip(weights) {
        // `total * weight` needs 512 bits; the quotient is at most `total`,
        // since `weight <= total_weight`, so it fits back in an Amount.
        let floor = total.mul_div(*weight, total_weight)?;
        *payout = floor;
        assigned = assigned.checked_add(floor)?;
    }

    // Each floor drops less than one unit, so the gap is below the holder
    // count, which `distribute` has already bounded by u32.
    let gap = total.checked_sub(assigned)?.to_u64()?;
    usize::try_from(gap).ok()
}

/// Pays one extra base unit to each of the first `spare` holders in `order`.
///
/// The second half of [`distribute`]. When `order` holds each index once (the
/// sort guarantees that, since it only permutes the identity), this adds
/// exactly `spare` in total and at most one unit to any holder, which is what
/// the proofs of this function state.
pub(crate) fn place_spare(payouts: &mut [Amount], order: &[u32], spare: usize) -> Option<()> {
    for index in order.iter().take(spare) {
        // A holder cannot be paid past the ceiling by one extra base unit
        // unless their floor was already MAX, which needs a total that large.
        let slot = payouts.get_mut(*index as usize)?;
        *slot = slot.checked_add(Amount::ONE)?;
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
pub const fn combined_balance(balance_a: Amount, balance_b: Amount) -> Option<Amount> {
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
    /// `public_out + fee`, or `held + public_in`, would exceed [`Amount::MAX`].
    Overflow,
    /// More value left the pool than it held.
    Underflow {
        /// Pool balance after crediting `public_in`.
        held: Amount,
        /// Total leaving the pool: `public_out + fee`.
        withdrawn: Amount,
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
    held: Amount,
    public_in: Amount,
    public_out: Amount,
    fee: Amount,
) -> Result<Amount, SettleError> {
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
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn a(value: u64) -> Amount {
        Amount::from_u64(value)
    }

    #[test]
    fn credit_reports_the_ceiling() {
        assert_eq!(credit(a(1), a(2)), Some(a(3)));
        assert_eq!(credit(Amount::MAX, Amount::ZERO), Some(Amount::MAX));
        assert_eq!(credit(Amount::MAX, Amount::ONE), None);
    }

    #[test]
    fn credit_carries_past_u64() {
        // The ceiling that ADR-046 removed: a u64 balance stopped here.
        let expected = Amount::from_u128(u128::from(u64::MAX) + 1);
        assert_eq!(credit(a(u64::MAX), Amount::ONE), Some(expected));
    }

    #[test]
    fn debit_reports_an_overdraft() {
        assert_eq!(debit(a(3), a(2)), Some(a(1)));
        assert_eq!(debit(a(3), a(3)), Some(Amount::ZERO));
        assert_eq!(debit(a(3), a(4)), None);
        assert_eq!(debit(Amount::ZERO, Amount::MAX), None);
    }

    #[test]
    fn advance_nonce_stops_at_the_ceiling() {
        assert_eq!(advance_nonce(0), Some(1));
        assert_eq!(advance_nonce(u64::MAX), None);
    }

    #[test]
    fn total_outputs_of_nothing_is_zero() {
        assert_eq!(total_outputs([]), Some(Amount::ZERO));
    }

    #[test]
    fn total_outputs_rejects_a_wrap_through_the_middle() {
        // The sum of these three is not representable. An implementation that
        // wrapped on the second addition would report `Some(MAX)` here.
        assert_eq!(total_outputs([Amount::MAX, Amount::ONE, Amount::MAX]), None);
    }

    #[test]
    fn total_outputs_sums_the_ordinary_case() {
        assert_eq!(total_outputs([a(1), a(2), a(3)]), Some(a(6)));
    }

    #[test]
    fn settle_pool_moves_value_both_ways() {
        assert_eq!(settle_pool(a(100), a(50), a(30), a(5)), Ok(a(115)));
        assert_eq!(
            settle_pool(Amount::ZERO, a(100), Amount::ZERO, Amount::ZERO),
            Ok(a(100))
        );
    }

    #[test]
    fn settle_pool_separates_underflow_from_overflow() {
        assert_eq!(
            settle_pool(a(10), Amount::ZERO, a(20), Amount::ZERO),
            Err(SettleError::Underflow {
                held: a(10),
                withdrawn: a(20)
            })
        );
        assert_eq!(
            settle_pool(Amount::ZERO, Amount::ZERO, Amount::MAX, Amount::ONE),
            Err(SettleError::Overflow)
        );
        assert_eq!(
            settle_pool(Amount::MAX, Amount::ONE, Amount::ZERO, Amount::ZERO),
            Err(SettleError::Overflow)
        );
    }

    #[test]
    fn settle_pool_reports_the_credited_balance_on_underflow() {
        // `held` in the error is the balance *after* `public_in` landed, which
        // is the number that makes the failure legible: it is what the pool
        // actually had when the withdrawal was attempted.
        assert_eq!(
            settle_pool(a(10), a(5), a(20), Amount::ZERO),
            Err(SettleError::Underflow {
                held: a(15),
                withdrawn: a(20)
            })
        );
    }
}

#[cfg(test)]
mod distribution_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn amounts(values: &[u64]) -> Vec<Amount> {
        values.iter().copied().map(Amount::from_u64).collect()
    }

    fn sum(values: &[Amount]) -> Amount {
        total_outputs(values.iter().copied()).expect("sum fits")
    }

    /// Runs a distribution and returns the payouts.
    fn split(total: Amount, weights: &[Amount]) -> Vec<Amount> {
        let mut payouts = vec![Amount::ZERO; weights.len()];
        let mut order = vec![0u32; weights.len()];
        distribute(total, weights, &mut payouts, &mut order).expect("distribute");
        payouts
    }

    fn split_u64(total: u64, weights: &[u64]) -> Vec<Amount> {
        split(Amount::from_u64(total), &amounts(weights))
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
                let payouts = split_u64(total, &weights);
                assert_eq!(
                    sum(&payouts),
                    Amount::from_u64(total),
                    "total {total} across {weights:?}"
                );
            }
        }
    }

    #[test]
    fn an_exact_division_leaves_no_remainder_to_place() {
        assert_eq!(split_u64(100, &[1, 1, 1, 1]), amounts(&[25, 25, 25, 25]));
        assert_eq!(split_u64(90, &[1, 2]), amounts(&[30, 60]));
    }

    #[test]
    fn the_spare_units_go_to_the_largest_remainders() {
        // 10 across weights 1,1,1: each is 3.333, so three floors of 3 and one
        // spare unit. Every remainder is equal, so the tie-break decides — and
        // it must decide the same way every time.
        assert_eq!(split_u64(10, &[1, 1, 1]), amounts(&[4, 3, 3]));

        // 7 across 1,1,1,1: floors of 1 each, three spare. Again all tied.
        assert_eq!(split_u64(7, &[1, 1, 1, 1]), amounts(&[2, 2, 2, 1]));

        // 10 across 1,1,4: floors are 1, 1, 6 — eight of ten placed, so *two*
        // spares, and every remainder is 4, so the two lowest indices take
        // them. Counting the spares wrong is easy and the sum catches it.
        assert_eq!(split_u64(10, &[1, 1, 4]), amounts(&[2, 2, 6]));
    }

    #[test]
    fn every_holder_lands_within_one_base_unit_of_their_exact_share() {
        // What largest-remainder buys over rounding down and keeping the dust.
        let weights: Vec<u64> = (1..=200u64).collect();
        let total = 1_000_003u64;
        let payouts = split_u64(total, &weights);
        let total_weight: u128 = weights.iter().map(|w| u128::from(*w)).sum();

        for (payout, weight) in payouts.iter().zip(&weights) {
            let exact = u128::from(total) * u128::from(*weight) / total_weight;
            let paid = payout.to_u128().expect("small");
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
        let first = split_u64(1_000, &weights);
        for _ in 0..20 {
            assert_eq!(split_u64(1_000, &weights), first);
        }
        // The spare units went to the lowest indices, in order.
        let spare = 1_000 % 64;
        for (index, payout) in first.iter().enumerate() {
            let expected = 1_000 / 64 + u64::from(index < spare);
            assert_eq!(*payout, Amount::from_u64(expected), "holder {index}");
        }
    }

    #[test]
    fn a_holder_with_no_weight_is_paid_nothing() {
        assert_eq!(split_u64(100, &[0, 1, 0, 1]), amounts(&[0, 50, 0, 50]));
    }

    #[test]
    fn ten_thousand_holders_still_sum_exactly() {
        // The block this subsystem exists for. Not a performance claim — a
        // correctness one: the sum has to be exact at the scale it will run at,
        // not only at the scale a hand-written case reaches.
        let weights: Vec<u64> = (0..10_000u64).map(|index| index % 97 + 1).collect();
        let total = 123_456_789u64;
        let payouts = split_u64(total, &weights);
        assert_eq!(sum(&payouts), Amount::from_u64(total));
        assert_eq!(payouts.len(), 10_000);
    }

    /// Every total up to 255 against every three-holder weighting up to 7 each:
    /// the payouts sum to the total, and each holder gets their exact share
    /// rounded down or one unit above it.
    ///
    /// Exhaustive over that domain, and through the whole of `distribute`,
    /// sort included. A Kani harness over the same domain did not finish in
    /// fifteen minutes: the solver has to decide a symbolic division per
    /// holder. Enumeration decides the same finite set of cases in well under a
    /// second. Placement of the spare is proved at every magnitude in
    /// `proofs.rs`.
    #[test]
    fn every_small_distribution_is_exact_and_fair() {
        const MAX_TOTAL: u64 = 255;
        const MAX_WEIGHT: u64 = 7;
        for total in 0..=MAX_TOTAL {
            for a in 0..=MAX_WEIGHT {
                for b in 0..=MAX_WEIGHT {
                    for c in 0..=MAX_WEIGHT {
                        check_exact_and_fair_u64(total, &[a, b, c]);
                    }
                }
            }
        }
    }

    /// The same two properties at the edges of `u64`, where `total * weight`
    /// needs 128 bits.
    #[test]
    fn large_distributions_are_exact_and_fair() {
        const TOTALS: [u64; 5] = [u64::MAX, u64::MAX - 1, 1 << 63, (1 << 63) - 1, 3];
        const WEIGHTS: [u64; 7] = [0, 1, 2, 3, u32::MAX as u64, 1 << 62, (1 << 62) + 1];
        for total in TOTALS {
            for a in WEIGHTS {
                for b in WEIGHTS {
                    for c in WEIGHTS {
                        check_exact_and_fair_u64(total, &[a, b, c]);
                    }
                }
            }
        }
        check_exact_and_fair_u64(u64::MAX, &[u64::MAX - 1, 1]);
        check_exact_and_fair_u64(u64::MAX, &[u64::MAX / 2, u64::MAX / 2 + 1]);
    }

    /// At the 256-bit ceiling, where `total * weight` needs all 512 bits of
    /// the wide product and a 256-bit one would wrap. Fairness is checked by
    /// reconstruction: `floor * total_weight + rem == total * weight`.
    #[test]
    fn distributions_at_the_256_bit_ceiling_are_exact() {
        let half = Amount::MAX.checked_div(Amount::from_u64(2)).expect("half");
        let cases: [(Amount, Vec<Amount>); 4] = [
            (
                Amount::MAX,
                vec![half, half.checked_add(Amount::ONE).expect("fits")],
            ),
            (
                Amount::MAX,
                vec![
                    Amount::MAX.checked_sub(Amount::ONE).expect("fits"),
                    Amount::ONE,
                ],
            ),
            (Amount::MAX, amounts(&[1, 1, 1])),
            (
                half,
                vec![half, Amount::from_u64(7), Amount::from_u64(u64::MAX)],
            ),
        ];
        for (total, weights) in cases {
            let payouts = split(total, &weights);
            assert_eq!(sum(&payouts), total, "{weights:?}");
            let total_weight = sum(&weights);
            for (payout, weight) in payouts.iter().zip(&weights) {
                let floor = total.mul_div(*weight, total_weight).expect("floor");
                assert!(
                    *payout == floor || Some(*payout) == floor.checked_add(Amount::ONE),
                    "paid {payout}, floor {floor}"
                );
            }
        }
    }

    fn check_exact_and_fair_u64(total: u64, weights: &[u64]) {
        let total_weight: u128 = weights.iter().map(|w| u128::from(*w)).sum();
        let weights_wide = amounts(weights);
        let mut payouts = vec![Amount::ZERO; weights.len()];
        let mut order = vec![0u32; weights.len()];
        let placed = distribute(
            Amount::from_u64(total),
            &weights_wide,
            &mut payouts,
            &mut order,
        );
        if total_weight == 0 {
            assert!(placed.is_none(), "zero weight must refuse");
            return;
        }
        assert!(placed.is_some(), "{total} over {weights:?}");
        assert_eq!(sum(&payouts), Amount::from_u64(total), "{weights:?}");
        for (payout, weight) in payouts.iter().zip(weights) {
            // u128 is exact here: total and weight are both u64.
            let exact = u128::from(total) * u128::from(*weight) / total_weight;
            let payout = payout.to_u128().expect("at most total");
            assert!(
                payout == exact || payout == exact + 1,
                "{total} over {weights:?}: paid {payout}, exact share {exact}"
            );
        }
    }

    #[test]
    fn a_distribution_with_no_holders_moves_nothing_and_refuses_a_total() {
        let mut payouts: Vec<Amount> = Vec::new();
        let mut order: Vec<u32> = Vec::new();
        assert!(distribute(Amount::ZERO, &[], &mut payouts, &mut order).is_some());
        // Paying a non-zero total to nobody would make the value vanish.
        assert!(distribute(Amount::ONE, &[], &mut payouts, &mut order).is_none());
    }

    #[test]
    fn weights_that_sum_to_zero_are_refused() {
        // There is nothing to be in proportion to, and dividing by the total
        // weight would be a division by zero.
        let mut payouts = vec![Amount::ZERO; 3];
        let mut order = vec![0u32; 3];
        assert!(
            distribute(
                Amount::from_u64(10),
                &amounts(&[0, 0, 0]),
                &mut payouts,
                &mut order
            )
            .is_none()
        );
    }

    #[test]
    fn mismatched_buffers_are_refused_rather_than_truncating() {
        let weights = amounts(&[1, 1, 1]);
        let mut short = vec![Amount::ZERO; 2];
        let mut order = vec![0u32; 3];
        assert!(distribute(Amount::from_u64(10), &weights, &mut short, &mut order).is_none());

        let mut payouts = vec![Amount::ZERO; 3];
        let mut short_order = vec![0u32; 2];
        assert!(
            distribute(
                Amount::from_u64(10),
                &weights,
                &mut payouts,
                &mut short_order
            )
            .is_none()
        );
    }

    #[test]
    fn weights_that_overflow_an_amount_are_refused() {
        let mut payouts = vec![Amount::ZERO; 2];
        let mut order = vec![0u32; 2];
        let weights = [Amount::MAX, Amount::ONE];
        assert!(distribute(Amount::from_u64(10), &weights, &mut payouts, &mut order).is_none());
    }
}
