//! Low-memory ML-DSA-65 (FIPS 204): the same bytes as `fips204`, a fraction
//! of the RAM.
//!
//! # Why it exists
//!
//! `fips204` keeps the whole matrix `Â` (k·ℓ = 30 polynomials, 30 KiB) and
//! every vector in memory; on this host its key generation and signing each
//! need a 192–256 KiB thread stack (`tests/memory_tests.rs`). A Ledger app has
//! 28–40 KiB of SRAM in total. This module computes the same functions while
//! holding at most a handful of 1 KiB polynomials:
//!
//! - `Â` is never stored: each entry is sampled as it is multiplied
//!   ([`sample::accumulate_a_times`]).
//! - `y`, `s1` and `s2` are regenerated from their seeds when needed rather
//!   than kept.
//! - `w = Â·y` is produced one row at a time. For the commitment each row's
//!   `w1` is absorbed straight into the challenge hash; for the hints the row
//!   is recomputed.
//!
//! The price is time — `ExpandMask` and the NTT run k·ℓ times per attempt
//! instead of ℓ, and each row of `w` is computed twice. That is the right
//! trade on a device, where RAM is the constraint and a signature is
//! approved by a human anyway.
//!
//! # How it is trusted
//!
//! Byte-for-byte equality with `fips204` over random keys and messages, and
//! the NIST ACVP ML-DSA-65 keyGen and sigGen vectors (`tests/lowmem_tests.rs`).
//! No function here has behaviour of its own to be trusted for.
//!
//! # Every key-dependent buffer is zeroized
//!
//! Not only the secret key. `ρ'`, `ρ''` and the polynomial scratch buffers hold
//! `s1`, `s2`, `t0` and the mask `y` in the clear at various points, and any of
//! those recovers the key. They are `Zeroizing`, so each is wiped when its
//! frame goes — including on the early returns of the rejection loop. A
//! hardware wallet's threat model includes reading SRAM off a seized device.

pub mod poly;
pub mod sample;

use sha3::Shake256;
use sha3::digest::{ExtendableOutput, Update, XofReader};
use zeroize::Zeroizing;

use poly::{GAMMA2, N, Poly, decompose, power2round};
use sample::{ETA, GAMMA1, accumulate_a_times, expand_mask, rej_bounded, sample_in_ball};

/// Rows of `Â`.
pub const K: usize = 6;
/// Columns of `Â`.
pub const L: usize = 5;
/// Public key bytes.
pub const PUBLIC_KEY_LEN: usize = 1952;
/// Secret key bytes.
pub const SECRET_KEY_LEN: usize = 4032;
/// Signature bytes.
pub const SIGNATURE_LEN: usize = 3309;

const OMEGA: usize = 55;
const BETA: i32 = 196; // τ·η
const C_TILDE: usize = 48; // 2λ/8, λ = 192

// Byte offsets (FIPS 204 Algorithms 22, 24, 26).
const T1_BYTES: usize = 320; // 256 × 10 bits
const S_BYTES: usize = 128; // 256 × 4 bits
const T0_BYTES: usize = 416; // 256 × 13 bits
const Z_BYTES: usize = 640; // 256 × 20 bits
const SK_S1: usize = 128;
const SK_S2: usize = SK_S1 + L * S_BYTES;
const SK_T0: usize = SK_S2 + K * S_BYTES;
const SIG_Z: usize = C_TILDE;
const SIG_H: usize = SIG_Z + L * Z_BYTES;

/// Give up after this many rejected attempts. Each is rejected with
/// probability about 0.8 for ML-DSA-65, so this is unreachable in practice;
/// it exists so that a bug is an error rather than a device that hangs.
const MAX_ATTEMPTS: u16 = 800;

/// The signer refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// No attempt passed the rejection checks within [`MAX_ATTEMPTS`].
    Exhausted,
}

/// Writes `value` into the `bits`-wide slot `index` of `buf`, LSB first.
/// `buf` must start zeroed; bits are ORed in.
fn put_bits(buf: &mut [u8], index: usize, bits: usize, value: u32) {
    for b in 0..bits {
        if (value >> b) & 1 == 1 {
            let pos = index * bits + b;
            buf[pos / 8] |= 1 << (pos % 8);
        }
    }
}

/// Reads the `bits`-wide slot `index` of `buf`, LSB first.
fn get_bits(buf: &[u8], index: usize, bits: usize) -> u32 {
    (0..bits).fold(0, |acc, b| {
        let pos = index * bits + b;
        acc | (u32::from((buf[pos / 8] >> (pos % 8)) & 1) << b)
    })
}

fn shake256(parts: &[&[u8]], out: &mut [u8]) {
    let mut shake = Shake256::default();
    for part in parts {
        shake.update(part);
    }
    shake.finalize_xof().read(out);
}

/// ML-DSA.KeyGen_internal (FIPS 204 Algorithm 6).
#[inline(never)]
pub fn keygen(xi: &[u8; 32], pk: &mut [u8; PUBLIC_KEY_LEN], sk: &mut [u8; SECRET_KEY_LEN]) {
    pk.fill(0);
    sk.fill(0);
    let mut seeds = Zeroizing::new([0u8; 128]);
    // `K` and `L` are single-digit constants.
    shake256(&[xi, &[K as u8, L as u8]], seeds.as_mut_slice());
    let (rho, rest) = seeds.split_at(32);
    let (rho_prime, key) = rest.split_at(64);
    // A 128-byte array splits into 32 + 64 + 32 by construction. `expect`
    // rather than a zero fallback: a broken invariant must be loud, not a
    // silently all-zero seed that mints a working-looking wrong key.
    let rho: [u8; 32] = rho.try_into().expect("32-byte rho");
    let rho_prime: Zeroizing<[u8; 64]> =
        Zeroizing::new(rho_prime.try_into().expect("64-byte rho'"));
    pk[..32].copy_from_slice(&rho);
    sk[..32].copy_from_slice(&rho);
    sk[32..64].copy_from_slice(key);

    // `s` holds s1 and s2 in the clear, one polynomial at a time.
    let mut s: Zeroizing<Poly> = Zeroizing::new([0; N]);
    for r in 0..K + L {
        rej_bounded(&rho_prime, r as u16, &mut s);
        let base = if r < L {
            SK_S1 + r * S_BYTES
        } else {
            SK_S2 + (r - L) * S_BYTES
        };
        for (n, &c) in s.iter().enumerate() {
            put_bits(&mut sk[base..base + S_BYTES], n, 4, (ETA - c) as u32);
        }
    }

    let mut acc: Zeroizing<Poly> = Zeroizing::new([0; N]);
    for i in 0..K {
        acc.fill(0);
        for j in 0..L {
            rej_bounded(&rho_prime, j as u16, &mut s);
            for c in s.iter_mut() {
                *c = poly::reduce(*c);
            }
            poly::ntt(&mut s);
            accumulate_a_times(&rho, i as u8, j as u8, &s, &mut acc);
        }
        poly::inv_ntt(&mut acc);
        rej_bounded(&rho_prime, (L + i) as u16, &mut s);
        let t1_at = 32 + i * T1_BYTES;
        let t0_at = SK_T0 + i * T0_BYTES;
        for n in 0..N {
            let (t1, t0) = power2round(poly::add(acc[n], poly::reduce(s[n])));
            put_bits(&mut pk[t1_at..t1_at + T1_BYTES], n, 10, t1 as u32);
            put_bits(
                &mut sk[t0_at..t0_at + T0_BYTES],
                n,
                13,
                ((1 << 12) - t0) as u32,
            );
        }
    }
    let mut tr = [0u8; 64];
    shake256(&[pk.as_slice()], &mut tr);
    sk[64..128].copy_from_slice(&tr);
}

/// `μ = H(tr ‖ M', 64)`, absorbed as the message arrives — so a device
/// signs a transaction it never holds whole.
pub struct MuHasher(Shake256);

impl MuHasher {
    /// For `ML-DSA.Sign` (pure, external): `M' = 0 ‖ |ctx| ‖ ctx ‖ M`.
    #[must_use]
    pub fn external(sk: &[u8; SECRET_KEY_LEN], ctx: &[u8]) -> Self {
        let mut hasher = Self::internal(sk);
        // A context is at most 255 bytes by FIPS 204; callers pass `b""`.
        hasher.0.update(&[0, ctx.len() as u8]);
        hasher.0.update(ctx);
        hasher
    }

    /// For `ML-DSA.Sign_internal`: `M' = M`.
    #[must_use]
    pub fn internal(sk: &[u8; SECRET_KEY_LEN]) -> Self {
        let mut shake = Shake256::default();
        shake.update(&sk[64..128]);
        Self(shake)
    }

    /// Absorbs the next piece of the message.
    pub fn update(&mut self, piece: &[u8]) {
        self.0.update(piece);
    }

    /// `μ`.
    #[must_use]
    pub fn finish(self) -> [u8; 64] {
        let mut mu = [0u8; 64];
        self.0.finalize_xof().read(&mut mu);
        mu
    }
}

/// Row `i` of `w = NTT⁻¹(Â ∘ NTT(y))` into `w`; `scratch` holds each `ŷ_j`.
fn w_row(
    rho: &[u8; 32],
    rho_pp: &[u8; 64],
    kappa: u16,
    i: usize,
    w: &mut Poly,
    scratch: &mut Poly,
) {
    w.fill(0);
    for j in 0..L {
        expand_mask(rho_pp, kappa + j as u16, scratch);
        poly::ntt(scratch);
        accumulate_a_times(rho, i as u8, j as u8, scratch, w);
    }
    poly::inv_ntt(w);
}

/// A small secret polynomial from `sk`, centred.
fn unpack_small(sk: &[u8; SECRET_KEY_LEN], at: usize, out: &mut Poly) {
    for (n, c) in out.iter_mut().enumerate() {
        *c = ETA - get_bits(&sk[at..at + S_BYTES], n, 4) as i32;
    }
}

/// `t0[i]` from `sk`, centred.
fn unpack_t0(sk: &[u8; SECRET_KEY_LEN], i: usize, out: &mut Poly) {
    let at = SK_T0 + i * T0_BYTES;
    for (n, c) in out.iter_mut().enumerate() {
        *c = (1 << 12) - get_bits(&sk[at..at + T0_BYTES], n, 13) as i32;
    }
}

/// ML-DSA.Sign_internal from `μ` (FIPS 204 Algorithm 7).
///
/// # Errors
///
/// [`Error::Exhausted`], which a correct implementation never returns.
#[inline(never)]
pub fn sign_mu(
    sk: &[u8; SECRET_KEY_LEN],
    mu: &[u8; 64],
    rnd: &[u8; 32],
    sig: &mut [u8; SIGNATURE_LEN],
) -> Result<(), Error> {
    let rho: [u8; 32] = sk[..32].try_into().expect("32-byte rho");
    // Derived from the secret key: a seed for every mask this signature uses.
    let mut rho_pp = Zeroizing::new([0u8; 64]);
    shake256(&[&sk[32..64], rnd, mu], rho_pp.as_mut_slice());

    let mut kappa: u16 = 0;
    for _ in 0..MAX_ATTEMPTS {
        if attempt(sk, &rho, &rho_pp, mu, kappa, sig) {
            return Ok(());
        }
        kappa += L as u16;
    }
    Err(Error::Exhausted)
}

/// One pass of the rejection loop; `true` if `sig` now holds a signature.
#[inline(never)]
fn attempt(
    sk: &[u8; SECRET_KEY_LEN],
    rho: &[u8; 32],
    rho_pp: &[u8; 64],
    mu: &[u8; 64],
    kappa: u16,
    sig: &mut [u8; SIGNATURE_LEN],
) -> bool {
    sig.fill(0);
    // These hold y, s1, s2, t0 and their products with the challenge.
    let (mut a, mut b, mut c): (Zeroizing<Poly>, Zeroizing<Poly>, Zeroizing<Poly>) =
        (Zeroizing::new([0; N]), Zeroizing::new([0; N]), Zeroizing::new([0; N]));

    // Commitment: c̃ = H(μ ‖ w1Encode(w1)), one row of w1 at a time.
    let mut commit = Shake256::default();
    commit.update(mu);
    let mut w1 = [0u8; S_BYTES];
    for i in 0..K {
        w_row(rho, rho_pp, kappa, i, &mut a, &mut b);
        w1.fill(0);
        for (n, &w) in a.iter().enumerate() {
            put_bits(&mut w1, n, 4, decompose(w).0 as u32);
        }
        commit.update(&w1);
    }
    let mut c_tilde = [0u8; C_TILDE];
    commit.finalize_xof().read(&mut c_tilde);
    let challenge = sample_in_ball(&c_tilde);
    sig[..C_TILDE].copy_from_slice(&c_tilde);

    // z = y + c·s1, packed as it is checked.
    for j in 0..L {
        expand_mask(rho_pp, kappa + j as u16, &mut a);
        unpack_small(sk, SK_S1 + j * S_BYTES, &mut b);
        challenge.times(&b, &mut c);
        let at = SIG_Z + j * Z_BYTES;
        for n in 0..N {
            let z = poly::centred(poly::add(a[n], poly::reduce(c[n])));
            if z.abs() >= GAMMA1 - BETA {
                return false;
            }
            put_bits(&mut sig[at..at + Z_BYTES], n, 20, (GAMMA1 - z) as u32);
        }
    }

    // Hints, recomputing each row of w.
    let mut hints = 0usize;
    for i in 0..K {
        w_row(rho, rho_pp, kappa, i, &mut a, &mut b);
        unpack_small(sk, SK_S2 + i * S_BYTES, &mut b);
        challenge.times(&b, &mut c); // c·s2
        for n in 0..N {
            a[n] = poly::sub(a[n], poly::reduce(c[n])); // r = w − c·s2
        }
        unpack_t0(sk, i, &mut b);
        challenge.times(&b, &mut c); // c·t0, exact and centred
        for n in 0..N {
            let (r1, r0) = decompose(a[n]);
            if r0.abs() >= GAMMA2 - BETA || c[n].abs() >= GAMMA2 {
                return false;
            }
            if decompose(poly::add(a[n], poly::reduce(c[n]))).0 != r1 {
                if hints == OMEGA {
                    return false;
                }
                sig[SIG_H + hints] = n as u8;
                hints += 1;
            }
        }
        sig[SIG_H + OMEGA + i] = hints as u8;
    }
    true
}

/// ML-DSA.Sign (pure) with `ctx`, over a whole message, with `rnd` given.
///
/// # Errors
///
/// As [`sign_mu`].
pub fn sign(
    sk: &[u8; SECRET_KEY_LEN],
    message: &[u8],
    ctx: &[u8],
    rnd: &[u8; 32],
    sig: &mut [u8; SIGNATURE_LEN],
) -> Result<(), Error> {
    let mut mu = MuHasher::external(sk, ctx);
    mu.update(message);
    sign_mu(sk, &mu.finish(), rnd, sig)
}
