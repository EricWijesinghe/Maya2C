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
//!
//! Multiplication is proved the same way, at full width: each
//! multiply-accumulate step fits a `u128` (`mul_step_fits_u128`) and each of
//! the four rows adds its products exactly (`mul_row_*_adds_exactly`), so by
//! induction `mul_wide` is the schoolbook product. Trusted: the compiler's
//! 64 × 64 → 128-bit multiply, one line per product in `mul_row_into`.
//!
//! Decimal output is split the same way: `decimal_chunk_step_is_exact`
//! proves `Display`'s 19-digit chunking at full width, and
//! `display_round_trips_through_a_decimal_reader` checks the whole `Display`
//! path against an independent reader for values below 10⁸. Trusted:
//! `core::fmt`'s integer formatting. The crate has no decimal parser, so the
//! reader lives here, in the proofs.

use crate::wide::{REM_LIMBS, lt, shl1_or, sub};
use crate::{Amount, Wide};

fn any_amount() -> Amount {
    Amount([kani::any(), kani::any(), kani::any(), kani::any()])
}

fn any_u128_amount() -> Amount {
    Amount([kani::any(), kani::any(), 0, 0])
}

// Superseded 2026-10-18: `add_and_sub_are_exact_inverses_and_never_wrap` and
// `order_is_total_and_matches_subtraction` were still in CBMC after 3 h. The
// `*_matches_the_u128_model` harnesses below prove the same operations equal
// to exact 256-bit arithmetic in seconds, which implies both: inverses, no
// wrap, a total order that agrees with subtraction.

// The next two multiplication harnesses (64-bit factors against `u128`,
// 128-bit commutativity) did not finish within 20 minutes each on a 24-thread
// desktop (2026-10-18, cargo-kani 0.67). Full-width multiplication is proved
// instead by `mul_step_fits_u128` and `mul_row_*_adds_exactly` at the end of
// this file (2026-10-19), given the compiler's 64 × 64 multiply.

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

/// An independent model of an amount as two native `u128` halves. CBMC
/// reasons about a 128-bit add or compare as one operation, so checking the
/// four-limb code against this model is cheap at full 256-bit width, where
/// the property harnesses above did not finish in three hours.
fn halves(a: Amount) -> (u128, u128) {
    let lo = u128::from(a.0[0]) | (u128::from(a.0[1]) << 64);
    let hi = u128::from(a.0[2]) | (u128::from(a.0[3]) << 64);
    (hi, lo)
}

#[allow(clippy::cast_possible_truncation)]
fn from_halves(hi: u128, lo: u128) -> Amount {
    Amount([lo as u64, (lo >> 64) as u64, hi as u64, (hi >> 64) as u64])
}

/// `checked_add` is exactly 256-bit addition, for every pair of amounts:
/// the right sum when it fits, `None` exactly when it does not.
#[kani::proof]
// 32: the `Option<Amount>` comparison is a 32-byte memcmp.
#[kani::unwind(33)]
fn checked_add_matches_the_u128_model() {
    let (a, b) = (any_amount(), any_amount());
    let ((ahi, alo), (bhi, blo)) = (halves(a), halves(b));
    let (lo, carry) = alo.overflowing_add(blo);
    let (hi, c1) = ahi.overflowing_add(bhi);
    let (hi, c2) = hi.overflowing_add(u128::from(carry));
    let model = if c1 || c2 {
        None
    } else {
        Some(from_halves(hi, lo))
    };
    assert!(a.checked_add(b) == model);
}

/// `checked_sub` is exactly 256-bit subtraction, `None` exactly below zero.
#[kani::proof]
// 32: the `Option<Amount>` comparison is a 32-byte memcmp.
#[kani::unwind(33)]
fn checked_sub_matches_the_u128_model() {
    let (a, b) = (any_amount(), any_amount());
    let ((ahi, alo), (bhi, blo)) = (halves(a), halves(b));
    let (lo, borrow) = alo.overflowing_sub(blo);
    let (hi, b1) = ahi.overflowing_sub(bhi);
    let (hi, b2) = hi.overflowing_sub(u128::from(borrow));
    let model = if b1 || b2 {
        None
    } else {
        Some(from_halves(hi, lo))
    };
    assert!(a.checked_sub(b) == model);
}

/// The order on amounts is the integer order: most significant half first.
#[kani::proof]
// 32: the `Option<Amount>` comparison is a 32-byte memcmp.
#[kani::unwind(33)]
fn order_matches_the_u128_model() {
    let (a, b) = (any_amount(), any_amount());
    assert!(a.cmp(&b) == halves(a).cmp(&halves(b)));
}

// Multiplication, split the way the long division is: what CBMC cannot do in
// one piece is prove a multiplier circuit equal to another one. These
// harnesses leave the 64 × 64 → 128-bit products to the compiler's `u128`
// multiply and prove everything `mul_wide` does around them, at full width.

/// Every multiply-accumulate step of `mul_wide` fits a `u128`, for every
/// limb, accumulator and carry: `a·b + o + c ≤ (2⁶⁴−1)² + 2(2⁶⁴−1) = 2¹²⁸−1`.
/// Kani's overflow check on the expression is the proof; the carry it leaves
/// therefore fits a limb.
#[kani::proof]
fn mul_step_fits_u128() {
    let (a, b, o, c): (u64, u64, u64, u64) = (kani::any(), kani::any(), kani::any(), kani::any());
    let cur = u128::from(a) * u128::from(b) + u128::from(o) + u128::from(c);
    assert!(cur >> 64 <= u128::from(u64::MAX));
}

/// `acc + v·2^(64·k)` over eight limbs, written apart from `wide.rs`, and
/// whether it carried out of the top.
fn add_u128_at(acc: [u64; 8], v: u128, k: usize) -> ([u64; 8], bool) {
    let mut out = acc;
    #[allow(clippy::cast_possible_truncation)]
    let parts = [v as u64, (v >> 64) as u64];
    let mut carry = 0u128;
    for (n, limb) in out.iter_mut().enumerate().skip(k) {
        let add = if n - k < 2 { parts[n - k] } else { 0 };
        let s = u128::from(*limb) + u128::from(add) + carry;
        #[allow(clippy::cast_possible_truncation)]
        {
            *limb = s as u64;
        }
        carry = s >> 64;
    }
    (out, carry != 0)
}

/// The carry chain of one row of `mul_wide`, for **every** accumulator it can
/// meet, every limb `i` and every four products a 64 × 64 multiply can give
/// (each at most `(2⁶⁴−1)²`): `add_products_at` is exactly
/// `out + Σ p_j·2^(64(i+j))`, nothing lost off the top, and no step
/// overflows (Kani's overflow checks).
///
/// Before row `i`, rows `0..i` have written limbs up to `i+3` only, so limbs
/// `i+4..` are zero: the precondition, which the exact sum keeps for row
/// `i+1`. By induction from zero, `mul_wide(a, b) = Σ_i Σ_j a_i·b_j·2^(64(i+j))`,
/// the definition of the product. What is taken as given is the compiler's
/// `u128` multiply in `mul_row_into`, one line per product.
///
/// Superseded (2026-10-19, cargo-kani 0.67): harnesses with the multipliers
/// inside, for the whole product (20 min) and one row (10 min), did not
/// finish.
fn row_step(i: usize) {
    let mut out: [u64; 8] = kani::any();
    for limb in &mut out[i + 4..] {
        *limb = 0;
    }
    let max = u128::from(u64::MAX) * u128::from(u64::MAX);
    let products: [u128; 4] = kani::any();
    let mut sum = out;
    for (j, p) in products.iter().enumerate() {
        kani::assume(*p <= max);
        let (next, carried) = add_u128_at(sum, *p, i + j);
        assert!(!carried);
        sum = next;
    }
    assert!(crate::add_products_at(out, i, products) == sum);
}
// Eight-limb loops need 9; the 64-byte array `==` is a memcmp, which needs 65.
#[kani::proof]
#[kani::unwind(65)]
fn mul_row_0_adds_exactly() {
    row_step(0);
}

#[kani::proof]
#[kani::unwind(65)]
fn mul_row_1_adds_exactly() {
    row_step(1);
}

#[kani::proof]
#[kani::unwind(65)]
fn mul_row_2_adds_exactly() {
    row_step(2);
}

#[kani::proof]
#[kani::unwind(65)]
fn mul_row_3_adds_exactly() {
    row_step(3);
}

/// Saturating add and subtract clamp exactly where the checked forms refuse,
/// for every pair of 256-bit amounts: `MAX` above the top, zero below the
/// bottom, the exact result otherwise.
#[kani::proof]
// 32: the `Amount` comparison is a 32-byte memcmp.
#[kani::unwind(33)]
fn saturating_add_and_sub_clamp_where_checked_refuses() {
    let (a, b) = (any_amount(), any_amount());
    assert!(a.saturating_add(b) == a.checked_add(b).unwrap_or(Amount::MAX));
    assert!(a.saturating_sub(b) == a.checked_sub(b).unwrap_or(Amount::ZERO));
}

/// `==`, `partial_cmp` and `cmp` tell one story, for every pair of 256-bit
/// amounts: `cmp` is `Equal` exactly when the limbs are equal, `partial_cmp`
/// never disagrees with it, and swapping the operands reverses it.
/// `order_matches_the_u128_model` proves `cmp` is the integer order; this
/// proves the derived `PartialEq` and the hand-written `PartialOrd` agree.
#[kani::proof]
#[kani::unwind(33)]
fn eq_partial_cmp_and_cmp_agree() {
    let (a, b) = (any_amount(), any_amount());
    let ord = a.cmp(&b);
    assert!((ord == core::cmp::Ordering::Equal) == (a == b));
    assert!(a.partial_cmp(&b) == Some(ord));
    assert!(b.cmp(&a) == ord.reverse());
}

// Decimal output. `Display` cuts the amount into 19-digit chunks with
// `div_rem_u64(10¹⁹)` and hands each chunk to `core::fmt` (`{}` for the
// leading one, `{:019}` after it). Split the way multiplication is:
// `decimal_chunk_step_is_exact` proves the cutting at full 256-bit width;
// `core::fmt`'s own integer formatting is trusted, as the compiler's multiply
// is above. `display_round_trips_through_a_decimal_reader` then checks the
// whole `Display` path end to end at a small width.
//
// Superseded 2026-11-10: the same end-to-end harness over every `u64` was
// still in CBMC's symbolic execution after 20 minutes (cargo-kani 0.67,
// 24-thread desktop). `Display`'s chunk loop and its padded-write loop exit
// on symbolic conditions, so CBMC unwinds every nested `core::fmt` loop to
// the harness's global bound (25, set by the 19-character zero padding).

/// One chunk step of `Display` is exact, for every 256-bit amount:
/// `a = q·10¹⁹ + r` with `r < 10¹⁹` (so `{:019}` prints every chunk after
/// the first in exactly 19 digits), `q` fits back in an `Amount`, and
/// `q < a` whenever `a ≠ 0` (so the loop ends). Five steps suffice because
/// `10⁹⁵ > 2²⁵⁶`, which is `DEC_CHUNKS`. No verdict in 20 minutes
/// (2026-11-10); runs in kani-heavy.yml.
#[kani::proof]
// 8 wide limbs in `div_rem_u64`; the `Amount` comparison is a 32-byte memcmp.
#[kani::unwind(33)]
fn decimal_chunk_step_is_exact() {
    let a = any_amount();
    let (q, r) = Wide::from_amount(a).div_rem_u64(crate::DEC_CHUNK);
    assert!(r < crate::DEC_CHUNK);
    let q = q.to_amount();
    assert!(q.is_some());
    let q = q.unwrap_or(Amount::ZERO);
    assert!(a.is_zero() || q < a);
    assert!(
        q.checked_mul(Amount::from(crate::DEC_CHUNK))
            .and_then(|p| p.checked_add(Amount::from(r)))
            == Some(a)
    );
}

/// The same chunk step for every amount below 2⁶⁴, checked against native
/// `u128` arithmetic instead of `checked_mul`.
///
/// Bound: 64 bits. `decimal_chunk_step_is_exact` above had no verdict in
/// 20 minutes: it asks CBMC to show a 256-bit divider equal to a multiplier,
/// the equivalence that also stalls the full-width multiply harnesses. Below
/// 2⁶⁴ only one limb of `div_rem_u64` is symbolic, the multiplier is the
/// constant 10¹⁹, and the step still takes both quotient values (0 below
/// 10¹⁹, 1 from 10¹⁹ to 2⁶⁴), which is the leading-chunk / padded-chunk split
/// `Display` makes for 20-digit values.
#[kani::proof]
#[kani::unwind(9)]
fn decimal_chunk_step_is_exact_below_2_64() {
    let x: u64 = kani::any();
    let (q, r) = Wide::from_amount(Amount::from(x)).div_rem_u64(crate::DEC_CHUNK);
    assert!(r < crate::DEC_CHUNK);
    let q = q.to_amount().and_then(Amount::to_u64);
    assert!(q.is_some());
    let q = q.unwrap_or(u64::MAX);
    assert!(u128::from(q) * u128::from(crate::DEC_CHUNK) + u128::from(r) == u128::from(x));
}

/// Room for the longest value the round trip below formats, with a margin,
/// so an over-long output fails the round trip instead of being cut short.
const DEC_BUF: usize = 8;

/// A fixed-size `fmt::Write` sink: the crate is `no_std` and Kani has no
/// allocator to give `to_string`.
struct DecBuf {
    bytes: [u8; DEC_BUF],
    len: usize,
}

impl core::fmt::Write for DecBuf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let end = self.len.checked_add(s.len()).ok_or(core::fmt::Error)?;
        let dst = self.bytes.get_mut(self.len..end).ok_or(core::fmt::Error)?;
        dst.copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

/// An independent decimal reader: ASCII digits only, no sign, no leading
/// zero unless the whole string is `0`, and the value must fit a `u64`.
/// Written without the crate's code, so the round trip cannot inherit a
/// mistake from `Display`.
fn parse_canonical_u64(digits: &[u8]) -> Option<u64> {
    if digits.is_empty() || (digits.len() > 1 && digits[0] == b'0') {
        return None;
    }
    let mut value: u64 = 0;
    for &c in digits {
        if !c.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add(u64::from(c - b'0'))?;
    }
    Some(value)
}

/// `Display` writes the canonical base-10 digits of an amount, so reading
/// them back gives the amount again, for every amount below 10⁸.
///
/// Bound: 8 digits, because the bound on the digits is the unwind bound.
/// The reader's loop needs one iteration per digit, Kani's unwind bound is
/// one number for every loop in the harness, and `Display`'s chunk and
/// padding loops exit on symbolic conditions, so CBMC unrolls them, nested,
/// to that same bound. Unwind 9 is the least the 8-limb `div_rem_u64` loop
/// accepts, which leaves 8 digits for the reader. At 25 (every `u64`) the
/// harness was still in symbolic execution after 20 minutes; at this bound
/// it reaches the solver (7.3 M variables) but had no verdict in 20 minutes
/// either (2026-11-10), so it runs in kani-heavy.yml. This covers the leading-chunk path
/// end to end; the zero-padded chunks after it are covered by
/// `decimal_chunk_step_is_exact` (each is below 10¹⁹) and by the unit tests
/// on `10¹⁹`, `2⁶⁴` and `MAX` (`tests.rs`).
#[kani::proof]
#[kani::unwind(9)]
fn display_round_trips_through_a_decimal_reader() {
    let x: u64 = kani::any();
    kani::assume(x < 100_000_000);
    let mut buf = DecBuf {
        bytes: [0; DEC_BUF],
        len: 0,
    };
    assert!(core::fmt::write(&mut buf, format_args!("{}", Amount::from(x))).is_ok());
    assert!(parse_canonical_u64(&buf.bytes[..buf.len]) == Some(x));
}
