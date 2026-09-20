//! Kani harnesses for every operation in this crate.
//!
//! Run them with:
//!
//! ```text
//! cargo kani -p maya-ledger-math
//! ```
//!
//! # How these are written
//!
//! Most harnesses check the operation against a **wider-type oracle**: the same
//! arithmetic performed in `u128`, where no `u64` ledger quantity can overflow.
//! That is a stronger statement than "the result did not wrap", because it pins
//! down what the answer must be rather than only what it must not be. A
//! `checked_add` replaced with a `saturating_add` still never wraps; it fails
//! the oracle immediately.
//!
//! # Bounded versus unbounded
//!
//! Stated plainly, because the difference is the difference between a proof and
//! a very good test:
//!
//! - [`credit`], [`debit`], [`advance_nonce`], [`combined_balance`] and
//!   [`settle_pool`] are proved **unbounded**. Their harnesses quantify over
//!   every `u64` input, with no loop and no unwind bound. There is no input
//!   these functions have not been checked on.
//! - [`total_outputs`] folds over a sequence, so its harness carries a
//!   `#[kani::unwind]` bound and is proved for sequences up to that length
//!   only. It is a **bounded** proof. The bound is set at
//!   [`PROVED_OUTPUT_COUNT`] below, which is smaller than the 65,536 outputs the
//!   wire codec will accept, and that gap is real rather than an oversight —
//!   see the constant's documentation.
//! - [`distribute`]'s placement of the spare is proved here, **bounded** at
//!   three holders but at every magnitude. Its flooring and sort are not: a
//!   harness through them has to decide a symbolic 128-bit division per holder
//!   and did not finish in fifteen minutes. Those are checked exhaustively over
//!   a small domain by `every_small_distribution_is_exact_and_fair` in the
//!   crate's unit tests instead, which is a finite enumeration, not a proof.
//!
//! [`credit`]: super::credit
//! [`debit`]: super::debit
//! [`advance_nonce`]: super::advance_nonce
//! [`combined_balance`]: super::combined_balance
//! [`settle_pool`]: super::settle_pool
//! [`total_outputs`]: super::total_outputs
//! [`distribute`]: super::distribute

use crate::{
    SettleError, advance_nonce, combined_balance, credit, debit, place_spare, settle_pool,
    total_outputs,
};

/// Output count the fold is proved over.
///
/// `ByteReader::read_collection_len` in the node accepts up to 65,536 elements,
/// so this proof does not cover every transaction the codec will decode. Four
/// is chosen because the fold is uniform — each step is one `checked_add` on
/// the running total, and the induction from step `n` to step `n + 1` carries no
/// new case — so a bound past the point where a second and third step have been
/// exercised buys confidence rather than coverage, at a cost that grows with the
/// bound.
///
/// The honest reading: the *step* is proved for all `u64`, and the *fold* is
/// proved up to four steps.
const PROVED_OUTPUT_COUNT: usize = 4;

/// `credit` returns the exact sum when it is representable, and `None`
/// precisely when it is not.
#[kani::proof]
fn credit_agrees_with_wide_addition() {
    let balance: u64 = kani::any();
    let amount: u64 = kani::any();

    let exact = u128::from(balance) + u128::from(amount);

    match credit(balance, amount) {
        Some(result) => {
            assert!(exact <= u128::from(u64::MAX));
            assert!(u128::from(result) == exact);
            // The two consequences a ledger actually relies on: money added is
            // money that did not go backwards.
            assert!(result >= balance);
            assert!(result >= amount);
        }
        None => assert!(exact > u128::from(u64::MAX)),
    }
}

/// `debit` returns the exact difference when it is non-negative, and `None`
/// precisely when the account is overdrawn.
#[kani::proof]
fn debit_agrees_with_wide_subtraction() {
    let balance: u64 = kani::any();
    let amount: u64 = kani::any();

    match debit(balance, amount) {
        Some(result) => {
            assert!(amount <= balance);
            assert!(u128::from(result) == u128::from(balance) - u128::from(amount));
            // The property that makes an overdraft safe: a debit never leaves
            // the account richer. This is the assertion that a wrapping
            // subtraction fails, and it is the worst bug a ledger can have.
            assert!(result <= balance);
        }
        None => assert!(amount > balance),
    }
}

/// Debiting then re-crediting the same amount restores the original balance.
///
/// Round-tripping is what rules out an off-by-one that both directions share
/// and that the oracle checks above would each accept on their own.
#[kani::proof]
fn debit_then_credit_is_the_identity() {
    let balance: u64 = kani::any();
    let amount: u64 = kani::any();

    if let Some(debited) = debit(balance, amount) {
        // Re-crediting cannot overflow: `debited + amount == balance`, which
        // was representable to begin with. Asserting that rather than assuming
        // it is the point of the harness.
        let restored = credit(debited, amount);
        assert!(restored == Some(balance));
    }
}

/// A debit strictly reduces the balance unless it moves nothing.
#[kani::proof]
fn debit_is_strict_unless_zero() {
    let balance: u64 = kani::any();
    let amount: u64 = kani::any();
    kani::assume(amount > 0);

    if let Some(result) = debit(balance, amount) {
        assert!(result < balance);
    }
}

/// `advance_nonce` steps by exactly one and stops only at the ceiling.
///
/// The ceiling case is unreachable in practice — 2^64 transactions from one
/// account — but a nonce that wrapped would make every transaction that account
/// ever sent replayable, so it is proved rather than argued.
#[kani::proof]
fn advance_nonce_steps_by_one() {
    let nonce: u64 = kani::any();

    match advance_nonce(nonce) {
        Some(next) => {
            assert!(nonce < u64::MAX);
            assert!(next == nonce + 1);
            assert!(next > nonce);
        }
        None => assert!(nonce == u64::MAX),
    }
}

/// `combined_balance` agrees with wide addition.
///
/// This is what a channel closure's claimed total is checked against before the
/// chain honours it. A wrap here would let two parties sign balances summing
/// past `u64::MAX`, present a small total to the capacity check, and mint the
/// difference.
#[kani::proof]
fn combined_balance_agrees_with_wide_addition() {
    let balance_a: u64 = kani::any();
    let balance_b: u64 = kani::any();

    let exact = u128::from(balance_a) + u128::from(balance_b);

    match combined_balance(balance_a, balance_b) {
        Some(total) => {
            assert!(u128::from(total) == exact);
            assert!(total >= balance_a);
            assert!(total >= balance_b);
        }
        None => assert!(exact > u128::from(u64::MAX)),
    }
}

/// `total_outputs` agrees with the exact `u128` sum of the same amounts.
///
/// **Bounded** at [`PROVED_OUTPUT_COUNT`] — see the module documentation.
///
/// The oracle is what makes this worth proving. The dangerous implementation is
/// not one that panics; it is one that wraps through the middle of the fold and
/// lands on a small, plausible total. `[u64::MAX, 1, u64::MAX]` sums to
/// `u64::MAX` under wrapping arithmetic, and a transaction reporting that total
/// would pass a balance check it should fail.
#[kani::proof]
#[kani::unwind(5)]
fn total_outputs_agrees_with_wide_summation() {
    let amounts: [u64; PROVED_OUTPUT_COUNT] = kani::any();

    let mut exact: u128 = 0;
    for amount in amounts {
        exact += u128::from(amount);
    }

    match total_outputs(amounts) {
        Some(total) => {
            assert!(u128::from(total) == exact);
            assert!(exact <= u128::from(u64::MAX));
        }
        None => assert!(exact > u128::from(u64::MAX)),
    }
}

/// The empty transaction moves nothing.
#[kani::proof]
fn total_outputs_of_nothing_is_zero() {
    let amounts: [u64; 0] = [];
    assert!(total_outputs(amounts) == Some(0));
}

/// A two-output transfer conserves value.
///
/// This is the composite property — the one that says the *sequence* in
/// `StateDB::stage_transaction` is sound, not merely that each step of it is.
/// The three accounts are modelled as distinct, which is the case that has to
/// hold; a self-transfer reads through the overlay and so sees its own debit,
/// which is a separate claim about the overlay rather than about arithmetic.
///
/// What is asserted: the total held across sender and both recipients is
/// identical before and after, computed in `u128` so the assertion itself
/// cannot be the thing that wraps.
#[kani::proof]
#[kani::unwind(3)]
fn a_two_output_transfer_conserves_value() {
    let sender_before: u64 = kani::any();
    let first_before: u64 = kani::any();
    let second_before: u64 = kani::any();
    let first_amount: u64 = kani::any();
    let second_amount: u64 = kani::any();

    // The node rejects the transaction unless every step below succeeds, so the
    // property is conditional on exactly that. Each `assume` mirrors a check in
    // `StateDB::stage_transaction`, in the order it performs them.
    let Some(total_out) = total_outputs([first_amount, second_amount]) else {
        return;
    };
    kani::assume(sender_before >= total_out);

    let Some(sender_after) = debit(sender_before, total_out) else {
        return;
    };
    let Some(first_after) = credit(first_before, first_amount) else {
        return;
    };
    let Some(second_after) = credit(second_before, second_amount) else {
        return;
    };

    let held_before =
        u128::from(sender_before) + u128::from(first_before) + u128::from(second_before);
    let held_after = u128::from(sender_after) + u128::from(first_after) + u128::from(second_after);

    assert!(held_before == held_after);
}

/// `settle_pool` agrees with exact wide arithmetic in all three outcomes.
///
/// The pool is the one balance on the chain whose contents nobody can audit
/// directly — that is what shielding means — so an arithmetic error here would
/// mint hidden supply rather than visible supply.
#[kani::proof]
fn settle_pool_agrees_with_wide_arithmetic() {
    let held: u64 = kani::any();
    let public_in: u64 = kani::any();
    let public_out: u64 = kani::any();
    let fee: u64 = kani::any();

    let ceiling = u128::from(u64::MAX);
    let withdrawn = u128::from(public_out) + u128::from(fee);
    let credited = u128::from(held) + u128::from(public_in);

    match settle_pool(held, public_in, public_out, fee) {
        Ok(remaining) => {
            assert!(withdrawn <= ceiling);
            assert!(credited <= ceiling);
            assert!(withdrawn <= credited);
            assert!(u128::from(remaining) == credited - withdrawn);
        }
        Err(SettleError::Overflow) => {
            assert!(withdrawn > ceiling || credited > ceiling);
        }
        Err(SettleError::Underflow {
            held: reported_held,
            withdrawn: reported_withdrawn,
        }) => {
            // Both sums were representable — otherwise this would be the
            // overflow arm — and the reported figures are the real ones, so the
            // tripwire says what actually happened rather than approximately
            // what happened.
            assert!(withdrawn <= ceiling);
            assert!(credited <= ceiling);
            assert!(withdrawn > credited);
            assert!(u128::from(reported_held) == credited);
            assert!(u128::from(reported_withdrawn) == withdrawn);
        }
    }
}

/// A settle that only deposits never reduces the pool, and one that only
/// withdraws never increases it.
///
/// Directionality, separate from the value check above: it is the property that
/// would break if `public_in` and `public_out` were ever transposed at a call
/// site, which the oracle harness alone would still accept.
#[kani::proof]
fn settle_pool_moves_value_in_the_stated_direction() {
    let held: u64 = kani::any();
    let amount: u64 = kani::any();

    if let Ok(after_deposit) = settle_pool(held, amount, 0, 0) {
        assert!(after_deposit >= held);
    }
    if let Ok(after_withdrawal) = settle_pool(held, 0, amount, 0) {
        assert!(after_withdrawal <= held);
    }
}

/// Placing the spare adds exactly the spare, and at most one unit to anyone.
///
/// The part of [`distribute`] that decides who is paid the dust. For any
/// `order` that holds each index once, which is what the sort returns,
/// placement adds exactly `spare` in total, so the floors plus the spare sum
/// to `total`: the invariant guard refuses any block whose value deltas do not
/// balance, so a rule that lost a unit here would make a distribution nobody
/// can put in a block. It also adds at most one unit
/// to each holder, so everyone ends at their exact share rounded down or one
/// above it. What largest-remainder buys over flooring and keeping the dust is
/// that nobody is short or over by more than a unit. A rule that met the sum
/// by paying one holder everything would add the right total and fail this.
///
/// Unbounded in magnitude: no division, so the payouts range over every `u64`.
#[kani::proof]
#[kani::unwind(4)]
fn placing_the_spare_adds_exactly_the_spare() {
    let before: [u64; 3] = kani::any();
    let order: [u32; 3] = kani::any();
    for index in order {
        kani::assume(index < 3);
    }
    kani::assume(order[0] != order[1] && order[0] != order[2] && order[1] != order[2]);
    let spare: usize = kani::any();
    kani::assume(spare < 3);

    let mut after = before;
    if place_spare(&mut after, &order, spare).is_some() {
        let sum = |payouts: &[u64; 3]| -> u128 { payouts.iter().map(|p| u128::from(*p)).sum() };
        assert!(sum(&after) == sum(&before) + spare as u128);
        for (was, now) in before.iter().zip(&after) {
            assert!(*now == *was || *now == *was + 1);
        }
    } else {
        // Refused only when a holder already had every unit a u64 can hold.
        assert!(before.contains(&u64::MAX));
    }
}
