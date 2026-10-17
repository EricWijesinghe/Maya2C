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
//! Amounts are 256-bit (ADR-046), so there is no wider primitive to use as an
//! oracle. Each harness instead pins the answer down by its **inverse**: a
//! credit that returns `r` must satisfy `r - amount == balance`, and one that
//! returns `None` must have `amount > MAX - balance`. That fixes what the
//! answer must be, not only what it must not be: a `checked_add` replaced with
//! a `saturating_add` still never wraps, and it fails the `None` arm
//! immediately. These harnesses take full 256-bit symbolic inputs; the
//! carry and borrow chains are also proved on their own in `maya-amount`.
//!
//! # Bounded versus unbounded
//!
//! Stated plainly, because the difference is the difference between a proof and
//! a very good test:
//!
//! - [`credit`], [`debit`], [`advance_nonce`], [`combined_balance`] and
//!   [`settle_pool`] are proved **unbounded**. Their harnesses quantify over
//!   every 256-bit input, with no loop and no unwind bound. There is no input
//!   these functions have not been checked on.
//! - [`total_outputs`] folds over a sequence, so its harness carries a
//!   `#[kani::unwind]` bound and is proved for sequences up to that length
//!   only. It is a **bounded** proof. The bound is set at
//!   [`PROVED_OUTPUT_COUNT`] below, which is smaller than the 65,536 outputs the
//!   wire codec will accept, and that gap is real rather than an oversight —
//!   see the constant's documentation.
//! - [`distribute`]'s placement of the spare is proved here, **bounded** at
//!   three holders but at every magnitude. Its flooring and sort are not: a
//!   harness through them has to decide a symbolic 512-by-256-bit division per holder
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
    Amount, SettleError, advance_nonce, combined_balance, credit, debit, place_spare, settle_pool,
    total_outputs,
};

/// Output count the fold is proved over.
///
/// `ByteReader::read_collection_len` in the node accepts up to 65,536 elements,
/// so this proof does not cover every transaction the codec will decode. Three
/// is chosen because the fold is uniform — each step is one `checked_add` on
/// the running total, and the induction from step `n` to step `n + 1` carries no
/// new case — so a bound past the point where a second and third step have been
/// exercised buys confidence rather than coverage, at a cost that grows with the
/// bound (each step is now a 256-bit carry chain).
///
/// The honest reading: the *step* is proved for every amount, and the *fold* is
/// proved up to three steps.
const PROVED_OUTPUT_COUNT: usize = 3;

fn any_amount() -> Amount {
    Amount(kani::any())
}

/// `a + b` is not representable exactly when `a > MAX - b`.
fn sum_overflows(a: Amount, b: Amount) -> bool {
    match Amount::MAX.checked_sub(b) {
        Some(room) => a > room,
        None => true,
    }
}

/// `credit` returns the exact sum when it is representable, and `None`
/// precisely when it is not.
#[kani::proof]
fn credit_agrees_with_its_inverse() {
    let balance = any_amount();
    let amount = any_amount();

    match credit(balance, amount) {
        Some(result) => {
            assert!(!sum_overflows(balance, amount));
            assert!(result.checked_sub(amount) == Some(balance));
            // The two consequences a ledger actually relies on: money added is
            // money that did not go backwards.
            assert!(result >= balance);
            assert!(result >= amount);
        }
        None => assert!(sum_overflows(balance, amount)),
    }
}

/// `debit` returns the exact difference when it is non-negative, and `None`
/// precisely when the account is overdrawn.
#[kani::proof]
fn debit_agrees_with_its_inverse() {
    let balance = any_amount();
    let amount = any_amount();

    match debit(balance, amount) {
        Some(result) => {
            assert!(amount <= balance);
            assert!(result.checked_add(amount) == Some(balance));
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
/// and that the inverse checks above would each accept on their own.
#[kani::proof]
fn debit_then_credit_is_the_identity() {
    let balance = any_amount();
    let amount = any_amount();

    if let Some(debited) = debit(balance, amount) {
        let restored = credit(debited, amount);
        assert!(restored == Some(balance));
    }
}

/// A debit strictly reduces the balance unless it moves nothing.
#[kani::proof]
fn debit_is_strict_unless_zero() {
    let balance = any_amount();
    let amount = any_amount();
    kani::assume(!amount.is_zero());

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

/// `combined_balance` agrees with its inverse.
///
/// This is what a channel closure's claimed total is checked against before the
/// chain honours it. A wrap here would let two parties sign balances summing
/// past the ceiling, present a small total to the capacity check, and mint the
/// difference.
#[kani::proof]
fn combined_balance_agrees_with_its_inverse() {
    let balance_a = any_amount();
    let balance_b = any_amount();

    match combined_balance(balance_a, balance_b) {
        Some(total) => {
            assert!(total.checked_sub(balance_b) == Some(balance_a));
            assert!(total >= balance_a);
            assert!(total >= balance_b);
        }
        None => assert!(sum_overflows(balance_a, balance_b)),
    }
}

/// `total_outputs` agrees with a step-by-step sum that stops at the first
/// overflow.
///
/// **Bounded** at [`PROVED_OUTPUT_COUNT`] — see the module documentation.
///
/// The dangerous implementation is not one that panics; it is one that wraps
/// through the middle of the fold and lands on a small, plausible total.
/// `[MAX, 1, MAX]` sums to `MAX` under wrapping arithmetic, and a transaction
/// reporting that total would pass a balance check it should fail.
#[kani::proof]
// 5: every 256-bit add is a four-limb carry loop, plus the three-output fold.
#[kani::unwind(5)]
fn total_outputs_refuses_any_intermediate_overflow() {
    let amounts: [Amount; PROVED_OUTPUT_COUNT] = [any_amount(), any_amount(), any_amount()];

    let mut running = Some(Amount::ZERO);
    for amount in amounts {
        running = match running {
            Some(sum) if !sum_overflows(sum, amount) => sum.checked_add(amount),
            _ => None,
        };
    }

    let total = total_outputs(amounts);
    assert!(total == running);
    if let Some(total) = total {
        for amount in amounts {
            assert!(total >= amount);
        }
    }
}

/// The empty transaction moves nothing.
#[kani::proof]
fn total_outputs_of_nothing_is_zero() {
    let amounts: [Amount; 0] = [];
    assert!(total_outputs(amounts) == Some(Amount::ZERO));
}

/// A two-output transfer conserves value.
///
/// This is the composite property — the one that says the *sequence* in
/// `StateDB::stage_transaction` is sound, not merely that each step of it is.
/// The three accounts are modelled as distinct, which is the case that has to
/// hold; a self-transfer reads through the overlay and so sees its own debit,
/// which is a separate claim about the overlay rather than about arithmetic.
///
/// What is asserted: the value that left the sender is exactly the value that
/// reached the recipients, each side measured by subtraction so the assertion
/// itself cannot be the thing that wraps.
#[kani::proof]
// 5: the four-limb carry and borrow loops in every Amount operation.
#[kani::unwind(5)]
fn a_two_output_transfer_conserves_value() {
    let sender_before = any_amount();
    let first_before = any_amount();
    let second_before = any_amount();
    let first_amount = any_amount();
    let second_amount = any_amount();

    // The node rejects the transaction unless every step below succeeds, so the
    // property is conditional on exactly that. Each step mirrors a check in
    // `StateDB::stage_transaction`, in the order it performs them.
    let Some(total_out) = total_outputs([first_amount, second_amount]) else {
        return;
    };
    let Some(sender_after) = debit(sender_before, total_out) else {
        return;
    };
    let Some(first_after) = credit(first_before, first_amount) else {
        return;
    };
    let Some(second_after) = credit(second_before, second_amount) else {
        return;
    };

    let left = sender_before.checked_sub(sender_after);
    let gained_first = first_after.checked_sub(first_before);
    let gained_second = second_after.checked_sub(second_before);
    let arrived = match (gained_first, gained_second) {
        (Some(a), Some(b)) => a.checked_add(b),
        _ => None,
    };
    assert!(left.is_some());
    assert!(left == arrived);
}

/// `settle_pool` agrees with its inverse in all three outcomes.
///
/// The pool is the one balance on the chain whose contents nobody can audit
/// directly — that is what shielding means — so an arithmetic error here would
/// mint hidden supply rather than visible supply.
#[kani::proof]
fn settle_pool_agrees_with_its_inverse() {
    let held = any_amount();
    let public_in = any_amount();
    let public_out = any_amount();
    let fee = any_amount();

    let out_overflows = sum_overflows(public_out, fee);
    let in_overflows = sum_overflows(held, public_in);

    match settle_pool(held, public_in, public_out, fee) {
        Ok(remaining) => {
            assert!(!out_overflows && !in_overflows);
            let withdrawn = public_out.checked_add(fee);
            let credited = held.checked_add(public_in);
            assert!(withdrawn.is_some() && credited.is_some());
            if let (Some(w), Some(c)) = (withdrawn, credited) {
                assert!(remaining.checked_add(w) == Some(c));
            }
        }
        Err(SettleError::Overflow) => {
            assert!(out_overflows || in_overflows);
        }
        Err(SettleError::Underflow {
            held: reported_held,
            withdrawn: reported_withdrawn,
        }) => {
            // Both sums were representable — otherwise this would be the
            // overflow arm — and the reported figures are the real ones, so the
            // tripwire says what actually happened rather than approximately
            // what happened.
            assert!(!out_overflows && !in_overflows);
            assert!(held.checked_add(public_in) == Some(reported_held));
            assert!(public_out.checked_add(fee) == Some(reported_withdrawn));
            assert!(reported_withdrawn > reported_held);
        }
    }
}

/// A settle that only deposits never reduces the pool, and one that only
/// withdraws never increases it.
///
/// Directionality, separate from the value check above: it is the property that
/// would break if `public_in` and `public_out` were ever transposed at a call
/// site, which the inverse harness alone would still accept.
#[kani::proof]
fn settle_pool_moves_value_in_the_stated_direction() {
    let held = any_amount();
    let amount = any_amount();

    if let Ok(after_deposit) = settle_pool(held, amount, Amount::ZERO, Amount::ZERO) {
        assert!(after_deposit >= held);
    }
    if let Ok(after_withdrawal) = settle_pool(held, Amount::ZERO, amount, Amount::ZERO) {
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
/// Unbounded in magnitude: no division, so the payouts range over every
/// 256-bit amount.
///
/// [`distribute`]: super::distribute
#[kani::proof]
// 5: the four-limb carry loop inside each placed unit.
#[kani::unwind(5)]
fn placing_the_spare_adds_exactly_the_spare() {
    let before: [Amount; 3] = [any_amount(), any_amount(), any_amount()];
    let order: [u32; 3] = kani::any();
    for index in order {
        kani::assume(index < 3);
    }
    kani::assume(order[0] != order[1] && order[0] != order[2] && order[1] != order[2]);
    let spare: usize = kani::any();
    kani::assume(spare < 3);

    let mut after = before;
    if place_spare(&mut after, &order, spare).is_some() {
        let mut added = 0usize;
        for (was, now) in before.iter().zip(&after) {
            if *now != *was {
                assert!(was.checked_add(Amount::ONE) == Some(*now));
                added += 1;
            }
        }
        assert!(added == spare);
    } else {
        // Refused only when a holder already had every unit an Amount holds.
        assert!(before.contains(&Amount::MAX));
    }
}
