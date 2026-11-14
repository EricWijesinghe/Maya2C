//! The CPU reference for every value the shader computes.
//!
//! Integer only (invariant 20's rule, though nothing here is consensus).
//! Every value is canonical, in `0..Q`, after every operation. `fips204`
//! keeps coefficients lazily reduced and signed; the shader cannot cheaply,
//! and a canonical value is unique, so equality of canonical words is
//! equality of the numbers `fips204` computes modulo `Q`. The loop structure
//! is `fips204::ntt`'s, line for line, so a reviewer can hold the two side by
//! side.

use crate::params::{F_MONT, N, NEG_QINV, Q, R2, ZETAS_MONT};

/// A polynomial: 256 canonical coefficients.
pub type Poly = [u32; N];

/// `a * b * 2^-32 mod Q` for `a, b < Q`.
///
/// Unsigned Montgomery reduction: `t + m*Q` is divisible by `2^32` when
/// `m = t * (-Q^-1) mod 2^32`, and the quotient is below `2Q` because
/// `t < Q^2 < Q * 2^32`.
#[must_use]
pub const fn mont_mul(a: u32, b: u32) -> u32 {
    let t = a as u64 * b as u64;
    let m = (t as u32).wrapping_mul(NEG_QINV);
    let r = ((t + m as u64 * Q as u64) >> 32) as u32;
    if r >= Q { r - Q } else { r }
}

/// `a + b mod Q`.
#[must_use]
pub const fn add_mod(a: u32, b: u32) -> u32 {
    let s = a + b;
    if s >= Q { s - Q } else { s }
}

/// `a - b mod Q`.
#[must_use]
pub const fn sub_mod(a: u32, b: u32) -> u32 {
    add_mod(a, Q - b)
}

/// `a * b mod Q`, plain domain: two Montgomery steps, the second by `2^64`.
#[must_use]
pub const fn mul_mod(a: u32, b: u32) -> u32 {
    mont_mul(mont_mul(a, b), R2)
}

/// FIPS 204 Algorithm 41, `NTT(w)`.
#[must_use]
pub fn ntt(w: &Poly) -> Poly {
    let mut out = *w;
    let mut m = 0;
    let mut len = 128;
    while len >= 1 {
        let mut start = 0;
        while start < N {
            m += 1;
            let zeta = ZETAS_MONT[m];
            for j in start..start + len {
                let t = mont_mul(zeta, out[j + len]);
                out[j + len] = sub_mod(out[j], t);
                out[j] = add_mod(out[j], t);
            }
            start += 2 * len;
        }
        len >>= 1;
    }
    out
}

/// FIPS 204 Algorithm 42, `NTT^-1(w_hat)`.
#[must_use]
pub fn inv_ntt(w_hat: &Poly) -> Poly {
    let mut out = *w_hat;
    let mut m = N;
    let mut len = 1;
    while len < N {
        let mut start = 0;
        while start < N {
            m -= 1;
            let neg_zeta = Q - ZETAS_MONT[m];
            for j in start..start + len {
                let t = out[j];
                out[j] = add_mod(t, out[j + len]);
                out[j + len] = mont_mul(neg_zeta, sub_mod(t, out[j + len]));
            }
            start += 2 * len;
        }
        len <<= 1;
    }
    for coefficient in &mut out {
        *coefficient = mont_mul(F_MONT, *coefficient);
    }
    out
}

/// Schoolbook multiplication in `Z_Q[X]/(X^256 + 1)`: the definition the
/// transform must agree with, kept for the tests.
#[must_use]
pub fn negacyclic_mul(a: &Poly, b: &Poly) -> Poly {
    let mut out = [0u32; N];
    for (i, &ai) in a.iter().enumerate() {
        for (j, &bj) in b.iter().enumerate() {
            let p = mul_mod(ai, bj);
            let k = i + j;
            if k < N {
                out[k] = add_mod(out[k], p);
            } else {
                out[k - N] = sub_mod(out[k - N], p);
            }
        }
    }
    out
}
