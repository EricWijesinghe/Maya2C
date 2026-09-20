//! The parameter set, and why it is borrowed rather than chosen.
//!
//! A lock is `t = A·s + e` over `R_q = Z_q[X]/(X^N + 1)`, with `A` a `K × L`
//! matrix of uniform polynomials, `s` a vector of `L` short polynomials and `e`
//! a vector of `K`. With `s` and `e` both drawn uniformly from `[-η, η]`, that
//! is exactly the public key ML-DSA-65 computes before it drops the low bits of
//! `t` — same `q`, same `N`, same ranks, same `η`. Finding *any* short opening
//! of a given `t` is the Module-LWE search problem at the parameters NIST
//! analysed for FIPS 204 category 3, and finding a *second* one is Module-SIS
//! with bound `2η`.
//!
//! Inventing a parameter set would have meant inventing a security argument.
//! Borrowing one means the argument already exists and this crate is only
//! responsible for not weakening it — which is what the bound check, the
//! canonical encodings and the trivial-commitment refusal are for.

/// Ring degree.
pub const N: usize = 256;

/// Modulus. ML-DSA's prime, `2^23 - 2^13 + 1`.
pub const Q: u32 = 8_380_417;

/// Rows of `A`: the length of `t` and of `e`.
pub const K: usize = 6;

/// Columns of `A`: the length of `s`.
pub const L: usize = 5;

/// Bound on every coefficient of an opening. `|c| ≤ ETA`.
pub const ETA: i32 = 4;

/// Bytes in the seed `A` is expanded from.
pub const SEED_BYTES: usize = 32;

/// Bytes in a commitment id or a lock id.
pub const DIGEST_BYTES: usize = 32;

/// Bits one reduced coefficient of `t` takes on the wire.
pub const COEFFICIENT_BITS: usize = 23;

/// Coefficients in `t`.
pub const COMMITMENT_COEFFICIENTS: usize = K * N;

/// Coefficients in an opening: `s`, then `e`.
pub const OPENING_COEFFICIENTS: usize = (L + K) * N;

/// Encoded commitment: the seed, then `t` packed at 23 bits.
pub const COMMITMENT_BYTES: usize = SEED_BYTES + COMMITMENT_COEFFICIENTS * COEFFICIENT_BITS / 8;

/// Encoded opening: one nibble per coefficient.
pub const OPENING_BYTES: usize = OPENING_COEFFICIENTS / 2;

// The encodings are exact, with no padding bits that could carry a second
// representation of the same value.
const _: () = {
    assert!(Q < 1 << COEFFICIENT_BITS);
    assert!((COMMITMENT_COEFFICIENTS * COEFFICIENT_BITS).is_multiple_of(8));
    assert!(OPENING_COEFFICIENTS.is_multiple_of(2));
    assert!(COMMITMENT_BYTES == 4_448);
    assert!(OPENING_BYTES == 1_408);
};
