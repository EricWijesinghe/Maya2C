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
//! commutativity, and divisors that fit in a `u64` for the round trip. The
//! 512-step long division of a wider divisor is proved one step at a time, at
//! full width: `long_division_step_keeps_the_invariant` shows each step keeps
//! `r < d` and adds one exact quotient bit, which by induction is the whole
//! division. The loop around the step is covered by the proptest suite
//! (`tests/property_tests.rs`).

use crate::wide::{REM_LIMBS, lt, shl1_or, sub};
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

/// The product is commutative for 128-bit factors. Limbs 2 and 3 are zero
/// here, so the upper partial products are not exercised; full-width
/// products are property-tested (`tests/property_tests.rs`), not proved.
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

/// Independent 5-limb adder for the step proof below: `a + b` and the carry
/// out. Written separately from `wide.rs` on purpose, so the proof checks
/// `shl1_or` and `sub` against arithmetic they do not share code with.
fn add5(a: [u64; REM_LIMBS], b: [u64; REM_LIMBS]) -> ([u64; REM_LIMBS], bool) {
    let mut out = [0u64; REM_LIMBS];
    let mut carry = 0u128;
    for i in 0..REM_LIMBS {
        let s = u128::from(a[i]) + u128::from(b[i]) + carry;
        #[allow(clippy::cast_possible_truncation)]
        {
            out[i] = s as u64;
        }
        carry = s >> 64;
    }
    (out, carry != 0)
}

/// One step of `Wide::long_div`, for **every** 256-bit divisor and every
/// remainder below it, with no bound on width. That is what lets the 512-step
/// loop be trusted without unrolling it.
///
/// With `r < d` on entry (true at the start, `r = 0`), the step computes
/// `r2 = 2r + bit` exactly, then either keeps it or subtracts `d`, and
/// leaves:
/// - `r' < d`, so the invariant holds for the next step;
/// - `r2 = q·d + r'` with `q` the quotient bit, so each step
///   contributes exactly one correct quotient bit.
///
/// By induction over the bits of the dividend, `n = q·d + r` with `r < d`.
// Five-limb loops need 6; the array `==` compares 40 bytes through memcmp,
// which needs 41.
#[kani::proof]
#[kani::unwind(41)]
fn long_division_step_keeps_the_invariant() {
    let d: [u64; REM_LIMBS] = [kani::any(), kani::any(), kani::any(), kani::any(), 0];
    // long_div only runs for divisors wider than 64 bits.
    kani::assume(d[1] != 0 || d[2] != 0 || d[3] != 0);
    let r: [u64; REM_LIMBS] = [
        kani::any(),
        kani::any(),
        kani::any(),
        kani::any(),
        kani::any(),
    ];
    kani::assume(lt(r, d));
    let bit: u64 = kani::any();
    kani::assume(bit <= 1);

    let r2 = shl1_or(r, bit);
    let (doubled, overflow) = add5(r, r);
    let (expected, overflow_bit) = add5(doubled, [bit, 0, 0, 0, 0]);
    assert!(!overflow && !overflow_bit);
    assert!(r2 == expected, "shl1_or is exactly 2r + bit");

    let (next, quotient_bit) = if lt(r2, d) {
        (r2, false)
    } else {
        (sub(r2, d), true)
    };
    assert!(lt(next, d), "the remainder stays below the divisor");
    if quotient_bit {
        assert!(add5(next, d) == (r2, false), "r2 = d + r'");
    } else {
        assert!(next == r2);
    }
}
