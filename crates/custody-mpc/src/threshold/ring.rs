//! Arithmetic in `R_q = Z_q[x] / (x^N + 1)`.
//!
//! # The matrix lives in the NTT domain, and that is not a detail
//!
//! The reference samples `A` *directly in the NTT domain* (`A_ntt =
//! _expand_a(seed)`): a uniform matrix is uniform either way, and sampling it
//! there saves a transform. So `A`'s coefficients are defined through the
//! reference's NTT -- its root, its twiddle order ([`super::ntt_table`]) --
//! and the first version of this port, which took the sampled values as
//! coefficients, computed a perfectly valid key that was not the reference's.
//! The known-answer test caught it. [`mat_vec`] therefore takes `A` as
//! sampled and multiplies the way the reference does.
//!
//! Everything else is in coefficient form. The challenge has only
//! [`OMEGA`](super::params::OMEGA) non-zero coefficients, so products with it
//! take the sparse [`mul_ternary`] path; [`mul`] is schoolbook and is kept as
//! the independent check the NTT is tested against.

use super::ntt_table::{N_INV, W};
use super::params::{N, Q};

/// A polynomial: `N` coefficients, each in `[0, modulus)`.
pub type Poly = Vec<u64>;
/// A vector of polynomials.
pub type PolyVec = Vec<Poly>;

fn mul_mod(a: u64, b: u64) -> u64 {
    // The product is below 2^98; the remainder is below Q < 2^49.
    u64::try_from((u128::from(a) * u128::from(b)) % u128::from(Q)).expect("below Q")
}

/// The zero polynomial.
#[must_use]
pub fn zero() -> Poly {
    vec![0; N]
}

/// `f + g mod m`.
#[must_use]
pub fn add(f: &[u64], g: &[u64], m: u64) -> Poly {
    f.iter().zip(g).map(|(a, b)| (a + b) % m).collect()
}

/// `f - g mod m`.
#[must_use]
pub fn sub(f: &[u64], g: &[u64], m: u64) -> Poly {
    f.iter()
        .zip(g)
        .map(|(a, b)| (a % m + m - b % m) % m)
        .collect()
}

/// `c · f mod Q` for a scalar `c`.
#[must_use]
pub fn scale(c: u64, f: &[u64]) -> Poly {
    f.iter().map(|&a| mul_mod(c, a)).collect()
}

/// `2^u · f mod Q` -- lifting a rounded value back to `Z_q`.
///
/// # Panics
///
/// If `u` is 64 or more.
#[must_use]
pub fn lshift(f: &[u64], u: u32) -> Poly {
    assert!(u < 64, "a shift of {u} bits");
    f.iter().map(|&a| mul_mod(a, 1u64 << u)).collect()
}

/// `round(f / 2^u) mod m`, the reference's `poly_rshift`.
///
/// # Panics
///
/// If `u` is 0 or 64 or more, or `m` is 0.
#[must_use]
pub fn rshift(f: &[u64], u: u32, m: u64) -> Poly {
    assert!(
        (1..64).contains(&u) && m > 0,
        "a rounding shift of {u} bits mod {m}"
    );
    let half = 1u64 << (u - 1);
    f.iter().map(|&a| ((a + half) >> u) % m).collect()
}

/// The coefficients of `f mod m`, centred into `(-m/2, m/2]`.
#[must_use]
pub fn centered(f: &[u64], m: u64) -> Vec<i128> {
    let mid = m >> 1;
    f.iter()
        .map(|&a| i128::from((a + mid) % m) - i128::from(mid))
        .collect()
}

/// A signed coefficient reduced into `[0, Q)`.
#[must_use]
pub fn from_signed(values: &[i64]) -> Poly {
    let q = i128::from(Q);
    values
        .iter()
        .map(|&v| u64::try_from(i128::from(v).rem_euclid(q)).expect("below Q"))
        .collect()
}

/// `f · g` in `R_q`, schoolbook, with `x^N = -1`.
#[must_use]
pub fn mul(f: &[u64], g: &[u64]) -> Poly {
    let mut acc = vec![0u128; N];
    let mut neg = vec![0u128; N];
    for (i, &a) in f.iter().enumerate() {
        if a == 0 {
            continue;
        }
        for (j, &b) in g.iter().enumerate() {
            let product = (u128::from(a) * u128::from(b)) % u128::from(Q);
            if i + j < N {
                acc[i + j] += product;
            } else {
                neg[i + j - N] += product;
            }
        }
    }
    // Each slot holds at most N products below Q, so it is below 2^58.
    let q = u128::from(Q);
    acc.iter()
        .zip(&neg)
        .map(|(p, n)| u64::try_from((p % q + q - n % q) % q).expect("below Q"))
        .collect()
}

/// `f · c` where `c` has coefficients in `{-1, 0, 1}` (a challenge).
#[must_use]
pub fn mul_ternary(f: &[u64], c: &[i8]) -> Poly {
    let mut out = zero();
    for (j, &cj) in c.iter().enumerate() {
        if cj == 0 {
            continue;
        }
        for (i, &a) in f.iter().enumerate() {
            // x^(i+j), folded by x^N = -1.
            let (slot, flip) = if i + j < N {
                (i + j, false)
            } else {
                (i + j - N, true)
            };
            let negate = (cj < 0) != flip;
            out[slot] = if negate {
                (out[slot] + Q - a) % Q
            } else {
                (out[slot] + a) % Q
            };
        }
    }
    out
}

/// `A · v` for a `K × L` matrix and an `L`-vector.
#[must_use]
pub fn mat_vec(a_ntt: &[PolyVec], v: &[Poly]) -> PolyVec {
    let v_ntt: PolyVec = v.iter().map(|p| ntt(p)).collect();
    a_ntt
        .iter()
        .map(|row| {
            let sum = row.iter().zip(&v_ntt).fold(zero(), |acc, (aij, vj)| {
                let product: Poly = aij.iter().zip(vj).map(|(&x, &y)| mul_mod(x, y)).collect();
                add(&acc, &product, Q)
            });
            intt(&sum)
        })
        .collect()
}

/// The reference's forward negacyclic NTT (`polyr.ntt`).
#[must_use]
pub fn ntt(f: &[u64]) -> Poly {
    let mut f = f.to_vec();
    let mut len = N / 2;
    let mut wi = 0;
    while len > 0 {
        for start in (0..N).step_by(2 * len) {
            wi += 1;
            let z = W[wi];
            for j in start..start + len {
                let (x, y) = (f[j], mul_mod(f[j + len], z));
                f[j] = (x + y) % Q;
                f[j + len] = (x + Q - y) % Q;
            }
        }
        len >>= 1;
    }
    f
}

/// The reference's inverse NTT (`polyr.intt`), including its `N^-1` scaling.
#[must_use]
pub fn intt(f: &[u64]) -> Poly {
    let mut f = f.to_vec();
    let mut len = 1;
    let mut wi = N;
    while len < N {
        for start in (0..N).step_by(2 * len) {
            wi -= 1;
            let z = W[wi];
            for j in start..start + len {
                let (x, y) = (f[j], f[j + len]);
                f[j] = (x + y) % Q;
                f[j + len] = mul_mod(z, (y + Q - x) % Q);
            }
        }
        len <<= 1;
    }
    f.iter().map(|&c| mul_mod(N_INV, c)).collect()
}

/// Componentwise `f + g`.
#[must_use]
pub fn vec_add(f: &[Poly], g: &[Poly], m: u64) -> PolyVec {
    f.iter().zip(g).map(|(a, b)| add(a, b, m)).collect()
}

/// Componentwise `f - g`.
#[must_use]
pub fn vec_sub(f: &[Poly], g: &[Poly], m: u64) -> PolyVec {
    f.iter().zip(g).map(|(a, b)| sub(a, b, m)).collect()
}

/// `x^{-1} mod Q` by the extended Euclidean algorithm, for the Lagrange
/// denominators (all below [`MAX_T`](super::params::MAX_T), so coprime to
/// both of `Q`'s prime factors).
#[must_use]
pub fn inverse(x: u64) -> u64 {
    let (mut r0, mut r1) = (i128::from(x % Q), i128::from(Q));
    let (mut s0, mut s1) = (1i128, 0i128);
    while r1 != 0 {
        let quotient = r0 / r1;
        (r0, r1) = (r1, r0 - quotient * r1);
        (s0, s1) = (s1, s0 - quotient * s1);
    }
    u64::try_from(s0.rem_euclid(i128::from(Q))).expect("below Q")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn poly(seed: u64) -> Poly {
        (0..N as u64)
            .map(|i| (i.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ seed) % Q)
            .collect()
    }

    #[test]
    fn x_to_the_n_is_minus_one() {
        let mut x = zero();
        x[1] = 1;
        let mut x_n_minus_1 = zero();
        x_n_minus_1[N - 1] = 1;
        let product = mul(&x, &x_n_minus_1);
        assert_eq!(product[0], Q - 1);
        assert!(product[1..].iter().all(|&c| c == 0));
    }

    #[test]
    fn the_sparse_product_is_the_dense_one() {
        let f = poly(7);
        let mut c = vec![0i8; N];
        for (k, i) in [3usize, 100, 511, 256, 0].into_iter().enumerate() {
            c[i] = if k % 2 == 0 { 1 } else { -1 };
        }
        let dense: Poly = c
            .iter()
            .map(|&v| match v {
                1 => 1,
                -1 => Q - 1,
                _ => 0,
            })
            .collect();
        assert_eq!(mul_ternary(&f, &c), mul(&f, &dense));
    }

    #[test]
    fn the_ntt_inverts_and_multiplies_negacyclically() {
        let (f, g) = (poly(3), poly(11));
        assert_eq!(intt(&ntt(&f)), f);
        let pointwise: Poly = ntt(&f)
            .iter()
            .zip(ntt(&g))
            .map(|(&a, b)| mul_mod(a, b))
            .collect();
        assert_eq!(
            intt(&pointwise),
            mul(&f, &g),
            "NTT product is the schoolbook one"
        );
    }

    #[test]
    fn inverses_invert() {
        for x in [1u64, 2, 3, 1023, 1024] {
            assert_eq!(mul_mod(x, inverse(x)), 1, "{x}");
        }
    }

    #[test]
    fn rounding_matches_the_reference_rule() {
        // ((x + 2^(u-1)) >> u) mod m, including the wrap at the top.
        assert_eq!(
            rshift(&[(1 << 39) - 1], 40, super::super::params::Q_W),
            vec![0]
        );
        assert_eq!(rshift(&[1 << 39], 40, super::super::params::Q_W), vec![1]);
        assert_eq!(rshift(&[Q - 1], 40, super::super::params::Q_W), vec![0]);
    }
}
