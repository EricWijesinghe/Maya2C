//! ML-DSA-65's constants, and nothing else.
//!
//! There is no parameter-set argument anywhere in this crate (threat model
//! G-7, invariant 4): the dimensions below are compiled into the CPU code and
//! written into the shader as WGSL constants by [`crate::shader::source`].
//! Pointing this code at ML-DSA-44 or -87 means editing this file.
//!
//! The arithmetic constants are the ones `fips204` 0.4 uses (`lib.rs`,
//! `helpers.rs`, `ntt.rs`): `Q`, `ZETA`, `QINV`, and `F_MONT` for the inverse
//! transform's final scaling. Each is checked at compile time below rather
//! than trusted as a literal.

/// Coefficients per polynomial.
pub const N: usize = 256;
/// The modulus, `2^23 - 2^13 + 1`.
pub const Q: u32 = 8_380_417;
/// A primitive 512th root of unity modulo `Q` (FIPS 204 §2.5).
pub const ZETA: u32 = 1753;
/// Rows of `A` (ML-DSA-65).
pub const K: usize = 6;
/// Columns of `A` (ML-DSA-65).
pub const L: usize = 5;
/// Dropped bits of `t`.
pub const D: u32 = 13;
/// Nonzero coefficients of the challenge polynomial.
pub const TAU: usize = 49;
/// `TAU * ETA`.
pub const BETA: i32 = 196;
/// Coefficient range of the response `z`.
pub const GAMMA1: i32 = 1 << 19;
/// Low-order rounding range.
pub const GAMMA2: i32 = (Q as i32 - 1) / 32;
/// Most hint bits a signature may carry.
pub const OMEGA: usize = 55;
/// Bytes of the commitment hash `c_tilde` (lambda / 4).
pub const C_TILDE_LEN: usize = 48;
/// Encoded public key length.
pub const PUBLIC_KEY_LEN: usize = 32 + K * 320;
/// Encoded signature length.
pub const SIGNATURE_LEN: usize = C_TILDE_LEN + L * 640 + OMEGA + K;
/// Bytes of `w1Encode` output: four bits per coefficient.
pub const W1_ENCODED_LEN: usize = K * N / 2;

/// `Q^-1 mod 2^32`, as `fips204::helpers::mont_reduce` writes it.
pub const QINV: u32 = 58_728_449;
/// `-Q^-1 mod 2^32`: the unsigned Montgomery reduction adds `m * Q` where the
/// signed one subtracts, so it needs the negated inverse.
pub const NEG_QINV: u32 = QINV.wrapping_neg();
/// `2^64 mod Q`. One Montgomery multiply by this undoes the `2^-32` the
/// previous one introduced, so a pointwise product comes out in the plain
/// domain.
pub const R2: u32 = ((1u128 << 64) % Q as u128) as u32;
/// `256^-1 * 2^32 mod Q`, `fips204::ntt::inv_ntt`'s `F_MONT`.
pub const F_MONT: u32 = ((8_347_681u128 << 32) % Q as u128) as u32;

const _: () = assert!(Q.wrapping_mul(QINV) == 1);
const _: () = assert!((8_347_681u64 * 256) % Q as u64 == 1);
const _: () = assert!(PUBLIC_KEY_LEN == 1952);
const _: () = assert!(SIGNATURE_LEN == 3309);
const _: () = assert!(GAMMA2 == 261_888);

/// `zeta^brv8(m) * 2^32 mod Q` for `m` in `0..256`, canonical in `0..Q`:
/// the table `fips204::helpers::ZETA_TABLE_MONT` holds, reduced to the
/// unsigned range the shader works in.
pub const ZETAS_MONT: [u32; N] = zetas_mont();

const fn zetas_mont() -> [u32; N] {
    let mut table = [0u32; N];
    let mut m = 0;
    while m < N {
        let exponent = (m as u8).reverse_bits() as u32;
        let mut power: u64 = 1;
        let mut e = 0;
        while e < exponent {
            power = power * ZETA as u64 % Q as u64;
            e += 1;
        }
        table[m] = ((power << 32) % Q as u64) as u32;
        m += 1;
    }
    table
}
