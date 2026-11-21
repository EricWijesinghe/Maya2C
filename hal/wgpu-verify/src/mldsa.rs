//! ML-DSA-65 verification split in three: what the CPU does before the lattice
//! step ([`prepare`]), the lattice step ([`crate::lattice`]), and what the CPU
//! does after it ([`finish`]). FIPS 204 Algorithm 8 (`ML-DSA.Verify_internal`)
//! with the external "pure" framing of Algorithm 3.
//!
//! This split is *not* the verifier. [`cpu_verdict`] is: `maya-crypto-pq`'s
//! own entry point, the one the ACVP tests pin. The split exists so a GPU can
//! do the middle, and every test checks that the split reaches the same answer
//! as `cpu_verdict` (ADR-137 O-6 A).

use maya_crypto_pq::suite::{self, SuiteId};
use sha3::digest::{ExtendableOutput, Update, XofReader};
use sha3::{Shake128, Shake256};

use crate::arith::Poly;
use crate::lattice::{LatticeInput, LatticeOutput};
use crate::params::{
    BETA, C_TILDE_LEN, D, GAMMA1, GAMMA2, K, L, N, OMEGA, PUBLIC_KEY_LEN, Q, SIGNATURE_LEN, TAU,
};

/// One signature to verify.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// Encoded ML-DSA-65 public key.
    pub public_key: Vec<u8>,
    /// The message `M`.
    pub message: Vec<u8>,
    /// FIPS 204 context string; empty for every Maya2C transaction.
    pub context: Vec<u8>,
    /// Encoded signature.
    pub signature: Vec<u8>,
}

/// Why [`prepare`] refused before the lattice step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reject {
    /// The public key or signature has the wrong length.
    Length,
    /// The context is longer than 255 bytes.
    Context,
    /// The hint is not a valid encoding (FIPS 204 Algorithm 21 returns ⊥).
    Hint,
}

/// The CPU's half-way state: the lattice input plus what [`finish`] needs.
#[derive(Clone, Debug)]
pub struct Prepared {
    /// What the lattice step reads.
    pub input: Box<LatticeInput>,
    c_tilde: [u8; C_TILDE_LEN],
    mu: [u8; 64],
    hint: [[bool; N]; K],
    z_norm_ok: bool,
}

/// The authoritative verdict: `maya_crypto_pq::suite::verify_with_context`.
#[must_use]
pub fn cpu_verdict(item: &Item) -> bool {
    suite::verify_with_context(
        SuiteId::MlDsa65,
        &item.public_key,
        &item.message,
        &item.context,
        &item.signature,
    )
    .is_ok()
}

/// Decoding, `ExpandA`, `mu` and `SampleInBall`: Algorithm 8 lines 1-8.
///
/// # Errors
///
/// A [`Reject`] where Algorithm 8 would return false before line 9.
pub fn prepare(item: &Item) -> Result<Prepared, Reject> {
    if item.public_key.len() != PUBLIC_KEY_LEN || item.signature.len() != SIGNATURE_LEN {
        return Err(Reject::Length);
    }
    let context_len = u8::try_from(item.context.len()).map_err(|_| Reject::Context)?;
    let (rho, t1_packed) = item.public_key.split_at(32);
    let (c_tilde_bytes, rest) = item.signature.split_at(C_TILDE_LEN);
    let (z_packed, hint_bytes) = rest.split_at(L * 640);

    let hint = hint_unpack(hint_bytes).ok_or(Reject::Hint)?;
    let z_signed: [[i32; N]; L] = core::array::from_fn(|l| {
        let raw = unpack(&z_packed[l * 640..(l + 1) * 640], 20);
        core::array::from_fn(|n| GAMMA1 - raw[n] as i32)
    });
    let z_norm_ok = z_signed.iter().flatten().all(|&v| v.abs() < GAMMA1 - BETA);
    let z = z_signed.map(|poly| poly.map(canonical));
    let t1 =
        core::array::from_fn(|k| unpack(&t1_packed[k * 320..(k + 1) * 320], 10).map(|v| v << D));

    let mut c_tilde = [0u8; C_TILDE_LEN];
    c_tilde.copy_from_slice(c_tilde_bytes);
    let mut tr = [0u8; 64];
    shake256(&[&item.public_key], &mut tr);
    let mut mu = [0u8; 64];
    shake256(
        &[&tr, &[0, context_len], &item.context, &item.message],
        &mut mu,
    );

    Ok(Prepared {
        input: Box::new(LatticeInput {
            z,
            c: sample_in_ball(&c_tilde),
            t1,
            a_hat: expand_a(rho),
        }),
        c_tilde,
        mu,
        hint,
        z_norm_ok,
    })
}

/// `UseHint`, `w1Encode`, the challenge hash and the norm check: lines 10-13.
#[must_use]
pub fn finish(prepared: &Prepared, lattice: &LatticeOutput) -> bool {
    let mut encoded = [0u8; K * N / 2];
    for k in 0..K {
        for i in 0..N / 2 {
            let lo = use_hint(prepared.hint[k][2 * i], lattice.w_approx[k][2 * i]);
            let hi = use_hint(prepared.hint[k][2 * i + 1], lattice.w_approx[k][2 * i + 1]);
            encoded[k * N / 2 + i] = (lo | (hi << 4)) as u8;
        }
    }
    let mut c_tilde = [0u8; C_TILDE_LEN];
    shake256(&[&prepared.mu, &encoded], &mut c_tilde);
    prepared.z_norm_ok && c_tilde == prepared.c_tilde
}

/// Whether every coefficient of `z` is strictly inside `GAMMA1 - BETA`.
#[must_use]
pub fn z_norm_ok(prepared: &Prepared) -> bool {
    prepared.z_norm_ok
}

fn canonical(v: i32) -> u32 {
    v.rem_euclid(Q as i32) as u32
}

fn shake256(parts: &[&[u8]], out: &mut [u8]) {
    let mut hasher = Shake256::default();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize_xof().read(out);
}

/// Little-endian bit unpacking of 256 `width`-bit values.
fn unpack(bytes: &[u8], width: u32) -> Poly {
    let mask = (1u64 << width) - 1;
    let mut out = [0u32; N];
    let (mut acc, mut bits, mut index) = (0u64, 0u32, 0usize);
    for &byte in bytes {
        acc |= u64::from(byte) << bits;
        bits += 8;
        while bits >= width && index < N {
            out[index] = (acc & mask) as u32;
            acc >>= width;
            bits -= width;
            index += 1;
        }
    }
    out
}

/// FIPS 204 Algorithm 21, `HintBitUnpack`. `None` is its ⊥.
fn hint_unpack(y: &[u8]) -> Option<[[bool; N]; K]> {
    let mut h = [[false; N]; K];
    let mut index = 0usize;
    for (i, row) in h.iter_mut().enumerate() {
        let end = usize::from(y[OMEGA + i]);
        if end < index || end > OMEGA {
            return None;
        }
        let first = index;
        while index < end {
            if index > first && y[index - 1] >= y[index] {
                return None;
            }
            row[usize::from(y[index])] = true;
            index += 1;
        }
    }
    y[index..OMEGA].iter().all(|&b| b == 0).then_some(h)
}

/// FIPS 204 Algorithm 29, `SampleInBall`, canonical (`-1` is `Q-1`).
fn sample_in_ball(c_tilde: &[u8; C_TILDE_LEN]) -> Poly {
    let mut hasher = Shake256::default();
    hasher.update(c_tilde);
    let mut reader = hasher.finalize_xof();
    let mut sign_bytes = [0u8; 8];
    reader.read(&mut sign_bytes);
    let signs = u64::from_le_bytes(sign_bytes);
    let mut c = [0u32; N];
    for (bit, i) in (N - TAU..N).enumerate() {
        let j = loop {
            let mut byte = [0u8; 1];
            reader.read(&mut byte);
            if usize::from(byte[0]) <= i {
                break usize::from(byte[0]);
            }
        };
        c[i] = c[j];
        c[j] = if (signs >> bit) & 1 == 1 { Q - 1 } else { 1 };
    }
    c
}

/// FIPS 204 Algorithm 32, `ExpandA`, via Algorithm 30, `RejNTTPoly`.
fn expand_a(rho: &[u8]) -> [[Poly; L]; K] {
    core::array::from_fn(|r| {
        core::array::from_fn(|s| {
            let mut hasher = Shake128::default();
            hasher.update(rho);
            hasher.update(&[s as u8, r as u8]);
            let mut reader = hasher.finalize_xof();
            let mut poly = [0u32; N];
            let mut filled = 0;
            while filled < N {
                let mut b = [0u8; 3];
                reader.read(&mut b);
                let v = u32::from(b[0]) | u32::from(b[1]) << 8 | u32::from(b[2] & 0x7f) << 16;
                if v < Q {
                    poly[filled] = v;
                    filled += 1;
                }
            }
            poly
        })
    })
}

/// FIPS 204 Algorithm 36, `Decompose`, for `r` in `0..Q`.
fn decompose(r: u32) -> (i32, i32) {
    let r = r as i32;
    let mut r0 = r % (2 * GAMMA2);
    if r0 > GAMMA2 {
        r0 -= 2 * GAMMA2;
    }
    if r - r0 == Q as i32 - 1 {
        (0, r0 - 1)
    } else {
        ((r - r0) / (2 * GAMMA2), r0)
    }
}

/// FIPS 204 Algorithm 40, `UseHint`.
fn use_hint(h: bool, r: u32) -> u32 {
    const M: i32 = (Q as i32 - 1) / (2 * GAMMA2);
    let (r1, r0) = decompose(r);
    let out = match (h, r0 > 0) {
        (false, _) => r1,
        (true, true) => (r1 + 1).rem_euclid(M),
        (true, false) => (r1 - 1).rem_euclid(M),
    };
    out as u32
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn decompose_wraps_q_minus_one_to_zero() {
        // The one input where `r - r0 == Q - 1`: Algorithm 36's special case.
        assert_eq!(decompose(Q - 1), (0, -1));
        assert_eq!(decompose(0), (0, 0));
    }

    #[test]
    fn use_hint_moves_r1_by_one_each_way_modulo_sixteen() {
        assert_eq!(use_hint(true, 1), 1);
        assert_eq!(use_hint(true, 0), 15);
        assert_eq!(use_hint(false, Q - 1), 0);
    }

    #[test]
    fn hint_unpack_refuses_a_count_past_omega_and_an_unsorted_row() {
        let mut y = [0u8; OMEGA + K];
        assert!(hint_unpack(&y).is_some());
        y[OMEGA] = u8::try_from(OMEGA + 1).unwrap();
        assert!(hint_unpack(&y).is_none());
        let mut y = [0u8; OMEGA + K];
        y[0] = 5;
        y[1] = 5;
        y[OMEGA..].fill(2);
        assert!(hint_unpack(&y).is_none(), "repeated index");
        y[1] = 6;
        assert!(hint_unpack(&y).is_some());
        y[2] = 1;
        assert!(hint_unpack(&y).is_none(), "nonzero padding");
    }
}
