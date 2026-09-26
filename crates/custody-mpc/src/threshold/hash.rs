//! Every XOF, hash, PRF and MAC Threshold Raccoon names, byte for byte as the authors'
//! reference (`thrc_core.py`) computes them. The domain headers, the index
//! widths and the order of fields are the reference's, because the vectors in
//! `tests/fixtures/traccoon_kat.json` are.

use sha3::digest::{ExtendableOutput, Update, XofReader};
use sha3::{Shake128, Shake256};

use super::params::{A_SEED_LEN, CRH, ELL, K, MAC_LEN, N, OMEGA, Q, Q_BITS};
use super::ring::{Poly, PolyVec};

/// Bytes per coefficient in hashed encodings and uniform sampling.
const COEFF_BYTES: usize = (Q_BITS as usize).div_ceil(8);

/// `_hdr8`: a domain byte and up to seven byte fields.
#[must_use]
pub fn hdr8(domain: u8, fields: &[u8]) -> [u8; 8] {
    let mut out = [0u8; 8];
    out[0] = domain;
    out[1..=fields.len()].copy_from_slice(fields);
    out
}

/// `_hdr24`: a domain byte, an 8-bit index, then two 24-bit little-endian
/// indexes.
///
/// # Panics
///
/// If `i` or `j` does not fit 24 bits. Every caller passes a party index
/// (below [`MAX_T`](super::params::MAX_T), checked at keygen) or a length
/// bounded by a checked signing set, so this is an invariant, not input
/// validation -- and truncating instead would let two indexes share a header.
#[must_use]
pub fn hdr24(domain: u8, i: usize, j: usize, k: u8) -> [u8; 8] {
    const LIMIT: usize = 1 << 24;
    assert!(i < LIMIT && j < LIMIT, "hdr24 indexes are 24-bit");
    let mut out = [0u8; 8];
    out[0] = domain;
    out[1] = k;
    let bytes = |x: usize| u32::try_from(x).expect("below 2^24").to_le_bytes();
    out[2..5].copy_from_slice(&bytes(i)[..3]);
    out[5..8].copy_from_slice(&bytes(j)[..3]);
    out
}

fn shake256(parts: &[&[u8]], out: &mut [u8]) {
    let mut xof = Shake256::default();
    for part in parts {
        xof.update(part);
    }
    xof.finalize_xof().read(out);
}

/// `_xof`: SHAKE256, `len` bytes.
#[must_use]
pub fn xof(parts: &[&[u8]], len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    shake256(parts, &mut out);
    out
}

/// `_xof_sample_q`: a uniform polynomial by rejection from SHAKE128.
#[must_use]
pub fn sample_uniform(parts: &[&[u8]]) -> Poly {
    let mut xof = Shake128::default();
    for part in parts {
        xof.update(part);
    }
    let mut reader = xof.finalize_xof();
    let mask = (1u64 << Q_BITS) - 1;
    let mut out = Vec::with_capacity(N);
    let mut bytes = [0u8; 8];
    while out.len() < N {
        reader.read(&mut bytes[..COEFF_BYTES]);
        let x = u64::from_le_bytes(bytes) & mask;
        if x < Q {
            out.push(x);
        }
    }
    out
}

/// `_expand_a`: the `K × L` matrix from its seed, **in the NTT domain**, as
/// the reference samples it (see [`super::ring`] for why that matters).
#[must_use]
pub fn expand_a(seed: &[u8; A_SEED_LEN]) -> Vec<PolyVec> {
    (0..K)
        .map(|i| {
            (0..ELL)
                .map(|j| {
                    let header = hdr8(
                        b'A',
                        &[u8::try_from(i).expect("K"), u8::try_from(j).expect("L")],
                    );
                    sample_uniform(&[&header, seed])
                })
                .collect()
        })
        .collect()
}

/// `_hash_vec(dat, vec)`: SHAKE256 of a header, `dat`, and every
/// coefficient as seven little-endian bytes (reduced mod `Q`).
#[must_use]
pub fn hash_coeffs(dat: &[u8], coeffs: &[u64]) -> [u8; CRH] {
    let header = hdr24(b'H', dat.len(), COEFF_BYTES * coeffs.len(), 0);
    let mut xof = Shake256::default();
    xof.update(&header);
    xof.update(dat);
    for &c in coeffs {
        xof.update(&(c % Q).to_le_bytes()[..COEFF_BYTES]);
    }
    let mut out = [0u8; CRH];
    xof.finalize_xof().read(&mut out);
    out
}

/// [`hash_coeffs`] over a vector of polynomials, flattened in order.
#[must_use]
pub fn hash_vec(dat: &[u8], v: &[Poly]) -> [u8; CRH] {
    let flat: Vec<u64> = v.iter().flatten().copied().collect();
    hash_coeffs(dat, &flat)
}

/// The session hash: `_hash_vec(sid ‖ mu, act)`.
#[must_use]
pub fn session_hash(sid: &[u8], mu: &[u8], act: &[usize]) -> [u8; CRH] {
    let dat = [sid, mu].concat();
    let parties: Vec<u64> = act.iter().map(|&i| i as u64).collect();
    hash_coeffs(&dat, &parties)
}

/// `_chal_poly`: a weight-[`OMEGA`] ternary polynomial from a challenge hash.
#[must_use]
pub fn challenge(c_hash: &[u8; CRH]) -> Vec<i8> {
    let mut xof = Shake256::default();
    xof.update(&hdr8(b'c', &[u8::try_from(OMEGA).expect("small")]));
    xof.update(c_hash);
    let mut reader = xof.finalize_xof();
    let mut c = vec![0i8; N];
    let mut weight = 0;
    let mut bytes = [0u8; 2];
    let index_mask = u16::try_from(N - 1).expect("N fits a u16");
    while weight < OMEGA {
        reader.read(&mut bytes);
        let x = u16::from_le_bytes(bytes);
        let index = usize::from((x >> 1) & index_mask);
        if c[index] == 0 {
            c[index] = if x & 1 == 1 { 1 } else { -1 };
            weight += 1;
        }
    }
    c
}

/// `_mask_prf(i, j, seed, seh)`: one party's share of a zero-sum mask.
#[must_use]
pub fn mask_prf(i: usize, j: usize, seed: &[u8], seh: &[u8; CRH]) -> PolyVec {
    (0..ELL)
        .map(|k| {
            let header = hdr24(b'm', i, j, u8::try_from(k).expect("L"));
            sample_uniform(&[&header, seed, seh])
        })
        .collect()
}

/// `_hash_ctrb_1`: what round 2's MACs authenticate.
#[must_use]
pub fn hash_round_one(
    seh: &[u8; CRH],
    act: &[usize],
    commits: &[([u8; CRH], PolyVec)],
) -> [u8; CRH] {
    let mut xof = Shake256::default();
    xof.update(seh);
    for (&_party, (cmt, mask)) in act.iter().zip(commits) {
        xof.update(&hash_vec(cmt, mask));
    }
    let mut out = [0u8; CRH];
    xof.finalize_xof().read(&mut out);
    out
}

/// `_sig_mac_ctrb_1(i, j, seed, h)`.
#[must_use]
pub fn mac(i: usize, j: usize, seed: &[u8], round_one: &[u8; CRH]) -> [u8; MAC_LEN] {
    let mut out = [0u8; MAC_LEN];
    shake256(&[&hdr24(b'M', i, j, 0), seed, round_one], &mut out);
    out
}
