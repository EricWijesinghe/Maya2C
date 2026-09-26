//! The FIPS 204 samplers, each a SHAKE stream consumed as it is squeezed.
//!
//! The one that matters for memory is [`accumulate_a_times`]: it samples one
//! entry of `Â` coefficient by coefficient and folds each straight into an
//! accumulator, so no entry of the 30 KiB matrix is ever held.

use sha3::digest::{ExtendableOutput, Update, XofReader};
use sha3::{Shake128, Shake256};

use super::poly::{self, Challenge, N, Poly, Q, TAU};

/// `η` for ML-DSA-65.
pub const ETA: i32 = 4;
/// `γ1 = 2^19` for ML-DSA-65.
pub const GAMMA1: i32 = 1 << 19;
/// Bytes of one `ExpandMask` polynomial: 256 coefficients × 20 bits.
pub const MASK_BYTES: usize = 640;

/// `acc += Â[row][col] ∘ ŷ`, sampling `Â[row][col]` on the fly
/// (FIPS 204 Algorithms 30 and 32; the seed is `ρ ‖ col ‖ row`).
pub fn accumulate_a_times(rho: &[u8; 32], row: u8, col: u8, yhat: &Poly, acc: &mut Poly) {
    let mut shake = Shake128::default();
    shake.update(rho);
    shake.update(&[col, row]);
    let mut reader = shake.finalize_xof();
    let mut j = 0;
    let mut three = [0u8; 3];
    while j < N {
        reader.read(&mut three);
        let coeff =
            i32::from(three[0]) | (i32::from(three[1]) << 8) | (i32::from(three[2] & 0x7f) << 16);
        if coeff < Q {
            acc[j] = poly::add(acc[j], poly::mul(coeff, yhat[j]));
            j += 1;
        }
    }
}

/// One polynomial of `s1` or `s2` (FIPS 204 Algorithm 31 with η = 4):
/// centred coefficients in `[−4, 4]`. `index` is `r` for `s1`, `r + ℓ` for
/// `s2`.
pub fn rej_bounded(rho_prime: &[u8; 64], index: u16, out: &mut Poly) {
    let mut shake = Shake256::default();
    shake.update(rho_prime);
    shake.update(&index.to_le_bytes());
    let mut reader = shake.finalize_xof();
    let mut j = 0;
    let mut byte = [0u8; 1];
    while j < N {
        reader.read(&mut byte);
        for half in [byte[0] & 0x0f, byte[0] >> 4] {
            if j < N && half < 9 {
                out[j] = ETA - i32::from(half);
                j += 1;
            }
        }
    }
}

/// One polynomial of `y` (FIPS 204 Algorithm 34), as canonical coefficients
/// ready for the NTT. `index` is `κ + r`.
pub fn expand_mask(rho_pp: &[u8; 64], index: u16, out: &mut Poly) {
    let mut shake = Shake256::default();
    shake.update(rho_pp);
    shake.update(&index.to_le_bytes());
    let mut reader = shake.finalize_xof();
    let mut five = [0u8; 5];
    for pair in out.as_chunks_mut::<2>().0 {
        reader.read(&mut five);
        let lo = u32::from(five[0]) | (u32::from(five[1]) << 8) | (u32::from(five[2] & 0x0f) << 16);
        let hi = u32::from(five[2] >> 4) | (u32::from(five[3]) << 4) | (u32::from(five[4]) << 12);
        // 20-bit values below 2^20, so the casts are exact.
        pair[0] = poly::reduce(GAMMA1 - lo as i32);
        pair[1] = poly::reduce(GAMMA1 - hi as i32);
    }
}

/// `c` from `c̃` (FIPS 204 Algorithm 29).
#[must_use]
pub fn sample_in_ball(c_tilde: &[u8]) -> Challenge {
    let mut shake = Shake256::default();
    shake.update(c_tilde);
    let mut reader = shake.finalize_xof();
    let mut signs = [0u8; 8];
    reader.read(&mut signs);
    let signs = u64::from_le_bytes(signs);

    let mut c = [0i8; N];
    let mut byte = [0u8; 1];
    for i in (N - TAU)..N {
        let j = loop {
            reader.read(&mut byte);
            if usize::from(byte[0]) <= i {
                break usize::from(byte[0]);
            }
        };
        c[i] = c[j];
        c[j] = if (signs >> (i + TAU - N)) & 1 == 1 {
            -1
        } else {
            1
        };
    }

    let mut challenge = Challenge {
        positions: [0; TAU],
        negative: [false; TAU],
    };
    let mut k = 0;
    for (position, &value) in c.iter().enumerate() {
        if value != 0 {
            // Exactly τ nonzero entries by construction, and N = 256.
            challenge.positions[k] = position as u8;
            challenge.negative[k] = value < 0;
            k += 1;
        }
    }
    challenge
}
