//! Threshold Raccoon's key generation, three signing rounds, combine and verify --
//! Algorithms 4-8 and Figure 3 of del Pino et al. (EUROCRYPT 2024), as the
//! authors' reference implements them (`thrc_core.py`).
//!
//! Parties are numbered from 0 and evaluated at `index + 1`, as there.

use std::collections::BTreeSet;

use zeroize::{Zeroize, ZeroizeOnDrop};

use super::gauss::Noise;
use super::hash::{self, hdr8, hdr24};
use super::params::{
    A_SEED_LEN, CRH, ELL, K, KEY_LEN, LG_SIGMA_T, LG_SIGMA_W_SQRT_T, MAC_LEN, MAX_T, MU_LEN, NU_T,
    NU_W, PAIR_SEED_LEN, Q, Q_T, Q_W, SID_LEN, b2,
};
use super::ring::{self, Poly, PolyVec};
use crate::error::CustodyError;

type Result<T> = core::result::Result<T, CustodyError>;

fn refuse(reason: &'static str) -> CustodyError {
    CustodyError::Lattice(reason)
}

/// The group's public key: the seed `A` expands from, and `t = round(A s + e)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifyingKey {
    /// Seed for `A`.
    pub a_seed: [u8; A_SEED_LEN],
    /// `round(A s + e)` in `R_{q_t}^k`.
    pub t: PolyVec,
}

/// One party's long-lived secret: its Shamir share of `s`, the pairwise seeds
/// it holds with every other party, and the session ids it has used.
///
/// # The used-session set must be as durable as the share
///
/// [`sign_1`] refuses a session id this share has answered before (the
/// paper's Remark 6.1). That set lives in memory. A service that stores
/// shares must persist it too, and commit each new id *before* releasing the
/// round-one message: a share reloaded without it would answer a replayed
/// session again.
#[derive(ZeroizeOnDrop)]
pub struct KeyShare {
    #[zeroize(skip)]
    index: usize,
    share: PolyVec,
    /// `(seed[index][j], seed[j][index])` for every party `j`.
    seeds: Vec<([u8; PAIR_SEED_LEN], [u8; PAIR_SEED_LEN])>,
    #[zeroize(skip)]
    used_sessions: BTreeSet<[u8; SID_LEN]>,
}

impl core::fmt::Debug for KeyShare {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("KeyShare")
            .field("index", &self.index)
            .field("share", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl KeyShare {
    /// Assembles a share. For [`super::dkg`], which builds them without a dealer.
    pub(crate) fn new(
        index: usize,
        share: PolyVec,
        seeds: Vec<([u8; PAIR_SEED_LEN], [u8; PAIR_SEED_LEN])>,
    ) -> Self {
        Self {
            index,
            share,
            seeds,
            used_sessions: BTreeSet::new(),
        }
    }

    /// This party's index.
    #[must_use]
    pub fn index(&self) -> usize {
        self.index
    }

    /// A digest of the share, for tests that must compare shares without
    /// printing them.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn share_digest(&self) -> [u8; 32] {
        super::digest(&self.share)
    }

    /// `seed[index][j]`, for the known-answer test. Crate-private: a pairwise
    /// seed is what keeps the column mask secret, and no caller outside this
    /// crate has any use for one.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn pair_seed(&self, j: usize) -> Option<[u8; PAIR_SEED_LEN]> {
        self.seeds.get(j).map(|(ours, _)| *ours)
    }
}

/// A signature: the challenge hash, the response, the hint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    /// `c = H_c(vk, mu, w)`.
    pub c_hash: [u8; CRH],
    /// `z` in `R_q^l`.
    pub z: PolyVec,
    /// `h` in `R_{q_w}^k`.
    pub h: PolyVec,
}

/// Round one's broadcast: a hash commitment to `w_j` and the public row mask.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoundOne {
    /// `H_com(sid, act, msg, w_j)`.
    pub cmt: [u8; CRH],
    /// `m_j`.
    pub mask: PolyVec,
}

/// Round two's broadcast: `w_j`, and a MAC over round one for every signer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoundTwo {
    /// The commitment share, unrounded.
    pub w: PolyVec,
    /// One MAC per member of `act`, in `act` order.
    pub macs: Vec<[u8; MAC_LEN]>,
}

/// One signer's state for one session.
#[derive(ZeroizeOnDrop)]
pub struct Session {
    #[zeroize(skip)]
    act: Vec<usize>,
    #[zeroize(skip)]
    mu: [u8; MU_LEN],
    #[zeroize(skip)]
    seh: [u8; CRH],
    r: PolyVec,
    #[zeroize(skip)]
    w: PolyVec,
    #[zeroize(skip)]
    round: u8,
    #[zeroize(skip)]
    round_one: Vec<RoundOne>,
}

/// Algorithm 4 with a **trusted dealer**, as the paper specifies it.
///
/// The dealer holds `s` while it runs, which is what the paper assumes and
/// what [`super::dkg`] exists to avoid. It is here because it is the
/// algorithm the known-answer vectors come from.
///
/// # Errors
///
/// A threshold of 0, above `parties`, or above [`MAX_T`].
#[allow(clippy::cast_precision_loss)]
pub fn keygen_dealer(
    key: &[u8; KEY_LEN],
    threshold: usize,
    parties: usize,
    noise: &mut dyn Noise,
) -> Result<(VerifyingKey, Vec<KeyShare>)> {
    check_roster(threshold, parties)?;
    let a_seed: [u8; A_SEED_LEN] = hash::xof(&[&hdr8(b'A', &[]), key], A_SEED_LEN)
        .try_into()
        .expect("A_SEED_LEN");
    let sigma_t2 = (1u64 << (2 * LG_SIGMA_T)) as f64;
    let s = draw(noise, sigma_t2, b's', &[], ELL, key);
    let mut e = draw(noise, sigma_t2, b'e', &[1], K, key);
    let t = public_key(&a_seed, &s, &e);
    e.zeroize();

    // Moved, not cloned: the secret's only copy becomes the constant term.
    let mut coefficients = vec![s];
    for i in 1..threshold {
        coefficients.push(
            (0..ELL)
                .map(|j| hash::sample_uniform(&[&hdr24(b'p', i, j, 0), key]))
                .collect(),
        );
    }
    let seed = |i: usize, j: usize| -> [u8; PAIR_SEED_LEN] {
        hash::xof(&[&hdr24(b'k', i, j, 0), key], PAIR_SEED_LEN)
            .try_into()
            .expect("PAIR_SEED_LEN")
    };
    let shares = (0..parties)
        .map(|i| {
            let seeds = (0..parties).map(|j| (seed(i, j), seed(j, i))).collect();
            KeyShare::new(i, evaluate(&coefficients, i as u64 + 1), seeds)
        })
        .collect();
    coefficients.zeroize();
    Ok((VerifyingKey { a_seed, t }, shares))
}

/// A roster the parameters and the headers can carry.
///
/// Two or more signers, at most [`MAX_T`] parties -- the bound the parameters
/// are proven for, and far inside the 24-bit indexes the domain headers use,
/// so two parties can never share a header. A threshold of one is refused: it
/// is a single-party key, and with one signer the round-one mask would be the
/// published one.
///
/// # Errors
///
/// A threshold below 2 or above `parties`, or more than [`MAX_T`] parties.
pub(crate) fn check_roster(threshold: usize, parties: usize) -> Result<()> {
    if threshold < 2 || threshold > parties || parties > MAX_T {
        return Err(refuse("the roster needs 2 <= threshold <= parties <= 1024"));
    }
    Ok(())
}

/// `t = round(A s + e)` at `NU_T`.
pub(crate) fn public_key(a_seed: &[u8; A_SEED_LEN], s: &[Poly], e: &[Poly]) -> PolyVec {
    let a = hash::expand_a(a_seed);
    let unrounded = ring::vec_add(&ring::mat_vec(&a, s), e, Q);
    unrounded
        .iter()
        .map(|p| ring::rshift(p, NU_T, Q_T))
        .collect()
}

/// Horner's rule for a polynomial whose coefficients are vectors in `R_q^l`.
pub(crate) fn evaluate(coefficients: &[PolyVec], x: u64) -> PolyVec {
    let mut acc = coefficients.last().expect("threshold >= 1").clone();
    for c in coefficients.iter().rev().skip(1) {
        acc = acc
            .iter()
            .zip(c)
            .map(|(a, ci)| ring::add(&ring::scale(x, a), ci, Q))
            .collect();
    }
    acc
}

/// `count` noise polynomials, seeded by `hdr8(domain, fields ‖ i) ‖ key`.
fn draw(
    noise: &mut dyn Noise,
    sigma2: f64,
    domain: u8,
    extra: &[u8],
    count: usize,
    key: &[u8],
) -> PolyVec {
    (0..count)
        .map(|i| {
            let mut fields = vec![u8::try_from(i).expect("small")];
            fields.extend_from_slice(extra);
            let seed = [&hdr8(domain, &fields)[..], key].concat();
            ring::from_signed(&noise.rounded(sigma2, &seed))
        })
        .collect()
}

fn check_signing_set(act: &[usize]) -> Result<()> {
    let increasing = act.windows(2).all(|pair| pair[0] < pair[1]);
    if act.is_empty() || !increasing || act.len() > MAX_T {
        return Err(refuse(
            "the signing set must be non-empty and strictly increasing",
        ));
    }
    Ok(())
}

fn check_act(act: &[usize], parties: usize, me: usize) -> Result<()> {
    check_signing_set(act)?;
    if act.last().is_some_and(|&last| last >= parties) || !act.contains(&me) {
        return Err(refuse(
            "the signing set must name this party and only known parties",
        ));
    }
    Ok(())
}

/// Domain for binding a signer's randomness to its session.
const NONCE_DOMAIN: &[u8] = b"maya2c.custody-mpc.traccoon.nonce-key.v1";

/// Algorithm 5, `ShareSign_1`, with the signer's randomness bound to the
/// session.
///
/// `key` must be fresh randomness for every call. It is nonetheless hashed
/// with the session hash (which covers `sid`, `mu` and `act`) before it seeds
/// `r_j`: if a caller ever reused a `key` across two sessions, the two `r_j`
/// would still differ. With the reference's derivation they would not, and
/// two responses under one `r_j` and different challenges give up the share
/// exactly as a reused Schnorr nonce does.
///
/// # Errors
///
/// A malformed signing set, or a session id this share has used before --
/// Remark 6.1: answering twice for one `sid` is how a threshold scheme leaks
/// its key, so the share refuses rather than trusting the caller.
pub fn sign_1(
    vk: &VerifyingKey,
    share: &mut KeyShare,
    sid: &[u8; SID_LEN],
    act: &[usize],
    mu: &[u8; MU_LEN],
    key: &[u8; KEY_LEN],
    noise: &mut dyn Noise,
) -> Result<(Session, RoundOne)> {
    // Before hashing it: the session hash's header has a 24-bit length field.
    check_signing_set(act)?;
    let seh = hash::session_hash(sid, mu, act);
    let bound: [u8; KEY_LEN] = hash::xof(&[NONCE_DOMAIN, key, &seh], KEY_LEN)
        .try_into()
        .expect("KEY_LEN");
    sign_1_reference(vk, share, sid, act, mu, &bound, noise)
}

/// `ShareSign_1` exactly as the reference derives it: `r_j` from `key` alone.
/// Crate-private and used only by the known-answer test; [`sign_1`] is the
/// entry point.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn sign_1_reference(
    vk: &VerifyingKey,
    share: &mut KeyShare,
    sid: &[u8; SID_LEN],
    act: &[usize],
    mu: &[u8; MU_LEN],
    key: &[u8; KEY_LEN],
    noise: &mut dyn Noise,
) -> Result<(Session, RoundOne)> {
    check_act(act, share.seeds.len(), share.index)?;
    if !share.used_sessions.insert(*sid) {
        return Err(refuse("this share has already signed in that session"));
    }
    let seh = hash::session_hash(sid, mu, act);
    let sigma_w2 = (1u128 << (2 * LG_SIGMA_W_SQRT_T)) as f64 / act.len() as f64;
    let r = draw(noise, sigma_w2, b'r', &[], ELL, key);
    let mut e2 = draw(noise, sigma_w2, b'e', &[2], K, key);
    let w = ring::vec_add(&ring::mat_vec(&hash::expand_a(&vk.a_seed), &r), &e2, Q);
    e2.zeroize();
    let cmt = hash::hash_vec(&seh, &w);
    let mask = sum(act
        .iter()
        .map(|&i| hash::mask_prf(share.index, i, &share.seeds[i].1, &seh)));
    let session = Session {
        act: act.to_vec(),
        mu: *mu,
        seh,
        r,
        w,
        round: 1,
        round_one: Vec::new(),
    };
    Ok((session, RoundOne { cmt, mask }))
}

fn sum(parts: impl Iterator<Item = PolyVec>) -> PolyVec {
    parts
        .reduce(|acc, x| ring::vec_add(&acc, &x, Q))
        .expect("a non-empty signing set")
}

/// Algorithm 6, `ShareSign_2`. `round_one` is every signer's broadcast, in
/// `act` order.
///
/// # Errors
///
/// Out-of-order rounds, or a broadcast set that does not match `act`.
pub fn sign_2(session: &mut Session, share: &KeyShare, round_one: &[RoundOne]) -> Result<RoundTwo> {
    if session.round != 1 || round_one.len() != session.act.len() {
        return Err(refuse(
            "round two needs round one's broadcast from every signer",
        ));
    }
    let digest = hash::hash_round_one(&session.seh, &session.act, &to_pairs(round_one));
    let macs = session
        .act
        .iter()
        .map(|&i| hash::mac(i, share.index, &share.seeds[i].0, &digest))
        .collect();
    session.round = 2;
    session.round_one = round_one.to_vec();
    Ok(RoundTwo {
        w: session.w.clone(),
        macs,
    })
}

fn to_pairs(round_one: &[RoundOne]) -> Vec<([u8; CRH], PolyVec)> {
    round_one.iter().map(|r| (r.cmt, r.mask.clone())).collect()
}

/// Algorithm 7, `ShareSign_3`. `round_two` is every signer's broadcast, in
/// `act` order.
///
/// # Errors
///
/// Out-of-order rounds; a revealed `w_i` that does not open its commitment;
/// or a MAC showing some signer saw a different round one. Either of the last
/// two aborts the session -- a signer never answers a view it cannot confirm.
pub fn sign_3(session: &mut Session, share: &KeyShare, round_two: &[RoundTwo]) -> Result<PolyVec> {
    if session.round != 2 || round_two.len() != session.act.len() {
        return Err(refuse(
            "round three needs round two's broadcast from every signer",
        ));
    }
    let me = position(&session.act, share.index);
    let digest = hash::hash_round_one(&session.seh, &session.act, &to_pairs(&session.round_one));
    for ((&i, first), second) in session.act.iter().zip(&session.round_one).zip(round_two) {
        if first.cmt != hash::hash_vec(&session.seh, &second.w) {
            return Err(refuse("a signer's commitment share does not open"));
        }
        let expected = hash::mac(share.index, i, &share.seeds[i].1, &digest);
        if second.macs.get(me) != Some(&expected) {
            return Err(refuse("a signer's view of round one differs from this one"));
        }
    }
    let w = aggregate_w(round_two.iter().map(|r| r.w.clone()));
    let c = hash::challenge(&hash::hash_vec(&session.mu, &w));
    let column_mask = sum(session
        .act
        .iter()
        .map(|&i| hash::mask_prf(i, share.index, &share.seeds[i].0, &session.seh)));
    let lambda = lagrange(&session.act, share.index);
    let z: PolyVec = share
        .share
        .iter()
        .zip(&session.r)
        .zip(&column_mask)
        .map(|((s, r), m)| {
            let cs = ring::scale(lambda, &ring::mul_ternary(s, &c));
            ring::add(&ring::add(&cs, r, Q), m, Q)
        })
        .collect();
    session.round = 3;
    session.r.zeroize();
    Ok(z)
}

fn position(act: &[usize], party: usize) -> usize {
    act.iter()
        .position(|&p| p == party)
        .expect("checked in sign_1")
}

fn aggregate_w(parts: impl Iterator<Item = PolyVec>) -> PolyVec {
    sum(parts)
        .iter()
        .map(|p| ring::rshift(p, NU_W, Q_W))
        .collect()
}

/// `lambda_{act,j} = prod_{i != j} -(i + 1) / (j - i)` mod `Q`, for `j` a
/// member of `act` (indices below [`MAX_T`]); for a non-member the product
/// is still computed but is not a Lagrange coefficient of `act`.
#[must_use]
pub fn lagrange(act: &[usize], j: usize) -> u64 {
    let q = i128::from(Q);
    let (mut num, mut den) = (1i128, 1i128);
    for &i in act.iter().filter(|&&i| i != j) {
        num = (num * -(i as i128 + 1)).rem_euclid(q);
        den = (den * (j as i128 - i as i128)).rem_euclid(q);
    }
    let den = u64::try_from(den).expect("below Q");
    let num = u64::try_from(num).expect("below Q");
    u64::try_from((u128::from(num) * u128::from(ring::inverse(den))) % u128::from(Q))
        .expect("below Q")
}

/// Algorithm 8, `Combine`, plus the reference's final bound check.
///
/// # Errors
///
/// Broadcasts that do not line up with `act`, or a combined signature over
/// the two-norm bound -- which is what fewer than `T` signers produce.
pub fn combine(
    vk: &VerifyingKey,
    mu: &[u8; MU_LEN],
    act: &[usize],
    round_one: &[RoundOne],
    round_two: &[RoundTwo],
    round_three: &[PolyVec],
) -> Result<Signature> {
    check_signing_set(act)?;
    let lengths = [round_one.len(), round_two.len(), round_three.len()];
    if lengths.iter().any(|&l| l != act.len()) {
        return Err(refuse("every round needs one broadcast per signer"));
    }
    let w = aggregate_w(round_two.iter().map(|r| r.w.clone()));
    let z_sum = sum(round_three.iter().cloned());
    let masks = sum(round_one.iter().map(|r| r.mask.clone()));
    let z = ring::vec_sub(&z_sum, &masks, Q);
    let c_hash = hash::hash_vec(mu, &w);
    let y = reconstruct_w(vk, &z, &c_hash);
    let h = ring::vec_sub(&w, &y, Q_W);
    let signature = Signature { c_hash, z, h };
    if !within_bound(&signature) {
        return Err(refuse("the combined signature exceeds the two-norm bound"));
    }
    Ok(signature)
}

/// `round(A z - 2^NU_T c t)` at `NU_W`.
fn reconstruct_w(vk: &VerifyingKey, z: &[Poly], c_hash: &[u8; CRH]) -> PolyVec {
    let c = hash::challenge(c_hash);
    let az = ring::mat_vec(&hash::expand_a(&vk.a_seed), z);
    az.iter()
        .zip(&vk.t)
        .map(|(azi, ti)| {
            let ct = ring::mul_ternary(&ring::lshift(ti, NU_T), &c);
            ring::rshift(&ring::sub(azi, &ct, Q), NU_W, Q_W)
        })
        .collect()
}

// Float arithmetic because the reference's is (the decision is made in
// Python floats there). Every integer converted here is exact in an f64:
// powers of two up to 2^84 and counts below 2^13.
#[allow(clippy::cast_precision_loss)]
fn within_bound(signature: &Signature) -> bool {
    let squares = |v: &[Poly], m: u64| -> u128 {
        v.iter()
            .flat_map(|p| ring::centered(p, m))
            .map(|x| u128::try_from(x * x).expect("a square"))
            .sum()
    };
    let total = squares(&signature.z, Q) + (1u128 << (2 * NU_W)) * squares(&signature.h, Q_W);
    (total as f64).sqrt() <= b2()
}

/// Figure 3's `Verify`.
#[must_use]
pub fn verify(vk: &VerifyingKey, mu: &[u8; MU_LEN], signature: &Signature) -> bool {
    let y = reconstruct_w(vk, &signature.z, &signature.c_hash);
    let w = ring::vec_add(&y, &signature.h, Q_W);
    hash::hash_vec(mu, &w) == signature.c_hash && within_bound(signature)
}
