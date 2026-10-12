//! Kani harnesses for [`Amount`] and [`Wide`].
//!
//! ```text
//! cargo kani -p maya-amount
//! ```
//!
//! Each harness checks one claim for **every** input of its stated width.
//! Add, subtract and compare are proved for all 256-bit values. The
//! multiply and divide harnesses bound the width where CBMC would otherwise
//! not finish: 64-bit factors against `u128`, 128-bit factors for
//! commutativity, and divisors that fit in a `u64` for the round trip. The 512-step long division of a wider divisor
//! is covered by the proptest suite (`tests/property_tests.rs`), not here.

use crate::{Amount, Wide};

fn any_amount() -> Amount {
    Amount([kani::any(), kani::any(), kani::any(), kani::any()])
}

fn any_u128_amount() -> Amount {
    Amount([kani::any(), kani::any(), 0, 0])
}

/// Add and subtract are exact inverses wherever they succeed, and fail
/// exactly where the true result leaves 0..2^256.
#[kani::proof]
#[kani::unwind(5)]
fn add_and_sub_are_exact_inverses_and_never_wrap() {
    let a = any_amount();
    let b = any_amount();
    match a.checked_add(b) {
        Some(sum) => {
            assert!(sum >= a && sum >= b);
            assert!(sum.checked_sub(b) == Some(a));
        }
        None => assert!(Amount::MAX.checked_sub(a).is_some_and(|room| b > room)),
    }
    match a.checked_sub(b) {
        Some(diff) => assert!(diff.checked_add(b) == Some(a)),
        None => assert!(a < b),
    }
}

/// The order is total, and agrees with subtraction: `a < b` exactly when
/// `a − b` borrows. Two independent code paths, so a wrong limb order in one
/// cannot hide behind the other.
#[kani::proof]
#[kani::unwind(5)]
fn order_is_total_and_matches_subtraction() {
    let a = any_amount();
    let b = any_amount();
    let lt = a < b;
    let eq = a == b;
    let gt = a > b;
    assert!(u8::from(lt) + u8::from(eq) + u8::from(gt) == 1);
    assert!(lt == a.checked_sub(b).is_none());
}

/// For 64-bit factors the product fits a `u128`, so it can be checked
/// exactly against the compiler's own multiplication; `checked_mul` agrees.
#[kani::proof]
#[kani::unwind(5)]
fn mul_wide_matches_u128_multiplication() {
    let x: u64 = kani::any();
    let y: u64 = kani::any();
    let w = Amount::from(x).mul_wide(Amount::from(y));
    let exact = u128::from(x) * u128::from(y);
    assert!(w == Wide::from_amount(Amount::from(exact)));
    assert!(Amount::from(x).checked_mul(Amount::from(y)) == Some(Amount::from(exact)));
}

/// The product is commutative for 128-bit factors, where every partial
/// product and carry of the schoolbook loop is exercised.
#[kani::proof]
#[kani::unwind(5)]
fn mul_wide_is_commutative() {
    let a = any_u128_amount();
    let b = any_u128_amount();
    assert!(a.mul_wide(b) == b.mul_wide(a));
}

/// Dividing `a × d` by `d` gives back `a` with no remainder, for every
/// 256-bit `a` and every non-zero divisor that fits in a `u64`; and every
/// division satisfies `q·d + r = n`, `r < d`.
#[kani::proof]
#[kani::unwind(9)]
fn mul_then_div_round_trips() {
    let a = any_amount();
    let d: u64 = kani::any();
    kani::assume(d != 0);
    let w = a.mul_wide(Amount::from(d));
    assert!(w.checked_div_rem(Amount::from(d)) == Some((a, Amount::ZERO)));

    let n = any_u128_amount();
    let (q, r) = Wide::from_amount(n)
        .checked_div_rem(Amount::from(d))
        .unwrap_or((Amount::ZERO, Amount::MAX));
    assert!(r < Amount::from(d));
    assert!(
        q.checked_mul(Amount::from(d))
            .and_then(|p| p.checked_add(r))
            == Some(n)
    );
}

/// A zero divisor is refused, never a panic.
#[kani::proof]
#[kani::unwind(5)]
fn division_by_zero_is_refused() {
    let a = any_amount();
    assert!(a.checked_div(Amount::ZERO).is_none());
    assert!(a.mul_div(any_amount(), Amount::ZERO).is_none());
}

/// Bytes are a bijection with amounts, low byte first.
// 4 limbs × 8 bytes, plus the exit check of each loop: the bound must clear
// the inner byte loop (8) and the slice compare in the assertion (32).
#[kani::proof]
#[kani::unwind(33)]
fn bytes_round_trip() {
    let a = any_amount();
    assert!(Amount::from_le_bytes(a.to_le_bytes()) == a);
    assert!(a.to_le_bytes()[0] == a.0[0].to_le_bytes()[0]);
}
