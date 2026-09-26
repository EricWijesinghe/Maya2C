//! Polynomial arithmetic for ML-DSA-65 (FIPS 204 §7.5–7.6): canonical
//! coefficients in `[0, q)`, the NTT and its inverse, rounding, and the
//! sparse product with the challenge.
//!
//! Plain modular arithmetic rather than Montgomery form. It is slower, and
//! this crate optimises for bytes of RAM and for being checkable against the
//! standard line by line; a Cortex-M33 has a single-cycle 32×32→64 multiply.

/// The modulus.
pub const Q: i32 = 8_380_417;
/// Coefficients per polynomial.
pub const N: usize = 256;
/// Dropped bits of `t` (FIPS 204 Table 1).
pub const D: u32 = 13;
/// `γ2 = (q − 1) / 32` for ML-DSA-65.
pub const GAMMA2: i32 = (Q - 1) / 32;

/// One polynomial: 1 KiB.
pub type Poly = [i32; N];

/// `a · b mod q`, both canonical.
#[must_use]
pub const fn mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) % Q as i64) as i32
}

/// `a + b mod q`, both canonical.
#[must_use]
pub const fn add(a: i32, b: i32) -> i32 {
    let s = a + b;
    if s >= Q { s - Q } else { s }
}

/// `a − b mod q`, both canonical.
#[must_use]
pub const fn sub(a: i32, b: i32) -> i32 {
    let s = a - b;
    if s < 0 { s + Q } else { s }
}

/// Any `i32` to its canonical representative.
#[must_use]
pub const fn reduce(a: i32) -> i32 {
    a.rem_euclid(Q)
}

/// The centred representative in `(−(q−1)/2, (q−1)/2]`, for norms.
#[must_use]
pub const fn centred(a: i32) -> i32 {
    if a > (Q - 1) / 2 { a - Q } else { a }
}

const fn pow_mod(mut base: i64, mut exp: u32) -> i64 {
    let mut acc = 1i64;
    base %= Q as i64;
    while exp > 0 {
        if exp & 1 == 1 {
            acc = acc * base % Q as i64;
        }
        base = base * base % Q as i64;
        exp >>= 1;
    }
    acc
}

const fn bit_rev8(k: usize) -> u32 {
    (k as u8).reverse_bits() as u32
}

/// `ζ^BitRev8(k)` with `ζ = 1753` (FIPS 204 Appendix B), computed at build
/// time rather than transcribed.
pub const ZETAS: [i32; N] = {
    let mut table = [0i32; N];
    let mut k = 0;
    while k < N {
        table[k] = pow_mod(1753, bit_rev8(k)) as i32;
        k += 1;
    }
    table
};

/// `256⁻¹ mod q`.
const N_INV: i32 = 8_347_681;

/// FIPS 204 Algorithm 41, in place.
pub fn ntt(w: &mut Poly) {
    let mut m = 0;
    let mut len = 128;
    while len >= 1 {
        let mut start = 0;
        while start < N {
            m += 1;
            let z = ZETAS[m];
            for j in start..start + len {
                let t = mul(z, w[j + len]);
                w[j + len] = sub(w[j], t);
                w[j] = add(w[j], t);
            }
            start += 2 * len;
        }
        len /= 2;
    }
}

/// FIPS 204 Algorithm 42, in place.
pub fn inv_ntt(w: &mut Poly) {
    let mut m = N;
    let mut len = 1;
    while len < N {
        let mut start = 0;
        while start < N {
            m -= 1;
            let z = Q - ZETAS[m];
            for j in start..start + len {
                let t = w[j];
                w[j] = add(t, w[j + len]);
                w[j + len] = mul(z, sub(t, w[j + len]));
            }
            start += 2 * len;
        }
        len *= 2;
    }
    for c in w.iter_mut() {
        *c = mul(*c, N_INV);
    }
}

/// `(r1, r0)` with `r = r1·2^d + r0` (FIPS 204 Algorithm 35), `r` canonical.
#[must_use]
pub const fn power2round(r: i32) -> (i32, i32) {
    let half = 1 << (D - 1);
    let mut r0 = r & ((1 << D) - 1);
    if r0 > half {
        r0 -= 1 << D;
    }
    ((r - r0) >> D, r0)
}

/// `(r1, r0)` (FIPS 204 Algorithm 36), `r` canonical.
#[must_use]
pub const fn decompose(r: i32) -> (i32, i32) {
    let two_gamma2 = 2 * GAMMA2;
    let mut r0 = r % two_gamma2;
    if r0 > GAMMA2 {
        r0 -= two_gamma2;
    }
    if r - r0 == Q - 1 {
        (0, r0 - 1)
    } else {
        ((r - r0) / two_gamma2, r0)
    }
}

/// The challenge `c`: τ = 49 positions and their signs.
pub struct Challenge {
    /// Nonzero positions.
    pub positions: [u8; TAU],
    /// `true` for −1.
    pub negative: [bool; TAU],
}

/// Nonzero coefficients of `c` for ML-DSA-65.
pub const TAU: usize = 49;

impl Challenge {
    /// `c · s` in the ring `Z_q[X]/(X^256 + 1)`, `s` given by its centred
    /// small coefficients; the result is centred too (|c·s|∞ ≤ τ·max|s|).
    pub fn times(&self, s: &Poly, out: &mut Poly) {
        out.fill(0);
        for (&p, &negative) in self.positions.iter().zip(&self.negative) {
            let p = usize::from(p);
            for (i, &si) in s.iter().enumerate() {
                let term = if negative { -si } else { si };
                let idx = i + p;
                if idx < N {
                    out[idx] += term;
                } else {
                    out[idx - N] -= term; // X^256 = −1
                }
            }
        }
    }
}
