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
