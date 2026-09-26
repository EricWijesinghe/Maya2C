//! Dealerless key generation for Threshold Raccoon. **Not peer-reviewed** -- this
//! construction is this repository's, and ADR-014 says so.
//!
//! # Why it exists
//!
//! The paper assumes a trusted dealer ("the design of a suitable DKG is
//! outside of the scope of this work"), and a dealer holds the whole key while
//! it runs. Master Prompt 2 §7 asks that the full key never exist in one
//! place. So each party contributes a *piece* of the secret and nobody ever
//! adds the pieces up:
//!
//! 1. Everyone agrees on `A` from a public ceremony id
//!    ([`matrix_seed`]) -- a nothing-up-my-sleeve derivation, since `A` is
//!    public and uniform in the paper.
//! 2. Party `p` draws `(s_p, e_p)` with variance `sigma_t^2 / N` each, so the
//!    sums have the paper's `sigma_t^2`, and computes `u_p = A s_p + e_p`.
//! 3. **Round 1** broadcasts a hash of `u_p` only. Committing first stops the
//!    last party from choosing its `u_p` after seeing everyone else's, which
//!    would let it set `t`.
//! 4. **Round 2** reveals `u_p`, and privately sends each party `i` a Shamir
//!    share `P_p(i + 1)` of `s_p` (degree `T - 1`) and the pairwise seed
//!    `seed[p][i]`.
//! 5. Party `i`'s share is `sum_p P_p(i + 1)`; the key is
//!    `t = round(sum_p u_p)`. The secret `s = sum_p s_p` is never formed.
//!
//! # What it does not defend against
//!
//! It is secure against parties who follow it and try to learn more than
//! their share. It is **not** verifiable: a dealer who sends inconsistent
//! shares is not caught by this protocol, and the result is a key that some
//! quorums cannot sign with -- a loss of availability, not a forgery, and
//! `tests/threshold_tests.rs` shows the failed signature is refused rather
//! than accepted. Lattice verifiable secret sharing that would catch it is an open
//! research area; ADR-014 records that as the gap.
//!
//! The summed secret is a sum of rounded Gaussians rather than one discrete
//! Gaussian; the paper's proof is for the latter. For widths this large the
//! two are statistically close, but it is outside what the proof covers.

use zeroize::{Zeroize, ZeroizeOnDrop};

use super::gauss::Noise;
use super::hash::{self, hdr8, hdr24};
use super::params::{A_SEED_LEN, CRH, ELL, K, KEY_LEN, LG_SIGMA_T, NU_T, PAIR_SEED_LEN, Q, Q_T};
use super::protocol::{KeyShare, VerifyingKey, check_roster, evaluate};
use super::ring::{self, PolyVec};
use crate::error::CustodyError;

type Result<T> = core::result::Result<T, CustodyError>;

/// Domain for the public matrix of one ceremony.
const MATRIX_DOMAIN: &[u8] = b"maya2c.custody-mpc.traccoon.dkg.matrix.v1";
/// Domain for the round-one commitment.
const COMMIT_DOMAIN: &[u8] = b"maya2c.custody-mpc.traccoon.dkg.commit.v1";

/// The `A` seed every party of ceremony `ceremony_id` uses.
#[must_use]
pub fn matrix_seed(ceremony_id: &[u8]) -> [u8; A_SEED_LEN] {
    hash::xof(&[MATRIX_DOMAIN, ceremony_id], A_SEED_LEN)
        .try_into()
        .expect("A_SEED_LEN")
}

/// Round one's broadcast: a commitment to `u_p`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Commitment(pub [u8; CRH]);

/// What party `p` sends party `i` in round two, privately.
#[derive(ZeroizeOnDrop)]
pub struct PrivateShare {
    share: PolyVec,
    seed: [u8; PAIR_SEED_LEN],
}

/// Round two's broadcast: the revealed `u_p`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reveal(pub PolyVec);

/// One party's view of a ceremony in progress.
///
/// The order is enforced, not assumed: nothing is revealed and no share is
/// dealt until [`Dealer::commitments_received`] has every party's commitment.
/// Committing before anyone reveals is what stops the last party choosing its
/// `u_p` to suit the others', and a method callable early would make that a
/// property of the caller rather than of the type.
#[derive(ZeroizeOnDrop)]
pub struct Dealer {
    #[zeroize(skip)]
    index: usize,
    #[zeroize(skip)]
    parties: usize,
    #[zeroize(skip)]
    a_seed: [u8; A_SEED_LEN],
    #[zeroize(skip)]
    u: PolyVec,
    coefficients: Vec<PolyVec>,
    seeds: Vec<[u8; PAIR_SEED_LEN]>,
    #[zeroize(skip)]
    commitments: Option<Vec<Commitment>>,
}

fn commit(index: usize, u: &[ring::Poly]) -> Commitment {
    let header = hdr24(b'D', index, 0, 0);
    Commitment(hash::hash_vec(&[COMMIT_DOMAIN, &header].concat(), u))
}

fn refuse(reason: &'static str) -> CustodyError {
    CustodyError::Lattice(reason)
}

/// `count` noise polynomials at variance `sigma2`, seeded by
/// `hdr8(domain, i) ‖ key`.
fn draw_piece(noise: &mut dyn Noise, sigma2: f64, domain: u8, count: usize, key: &[u8]) -> PolyVec {
    (0..count)
        .map(|i| {
            let seed = [&hdr8(domain, &[u8::try_from(i).expect("small")])[..], key].concat();
            ring::from_signed(&noise.rounded(sigma2, &seed))
        })
        .collect()
}

impl Dealer {
    /// Party `index`'s contribution, from 32 bytes of its own key material.
    ///
    /// # Errors
    ///
    /// A roster [`check_roster`] refuses, or an index outside `0..parties`.
    #[allow(clippy::cast_precision_loss)]
    pub fn begin(
        ceremony_id: &[u8],
        index: usize,
        threshold: usize,
        parties: usize,
        key: &[u8; KEY_LEN],
        noise: &mut dyn Noise,
    ) -> Result<(Self, Commitment)> {
        check_roster(threshold, parties)?;
        if index >= parties {
            return Err(refuse("this party's index is outside the roster"));
        }
        let a_seed = matrix_seed(ceremony_id);
        // Each piece at sigma_t^2 / N, so the N-fold sum has the paper's width.
        let sigma2 = (1u64 << (2 * LG_SIGMA_T)) as f64 / parties as f64;
        let s = draw_piece(noise, sigma2, b's', ELL, key);
        let mut e = draw_piece(noise, sigma2, b'e', K, key);
        let u = ring::vec_add(&ring::mat_vec(&hash::expand_a(&a_seed), &s), &e, Q);
        e.zeroize();
        let mut coefficients = vec![s];
        for c in 1..threshold {
            coefficients.push(
                (0..ELL)
                    .map(|j| hash::sample_uniform(&[&hdr24(b'p', c, j, 0), key]))
                    .collect(),
            );
        }
        let seeds = (0..parties)
            .map(|j| {
                hash::xof(&[&hdr24(b'k', index, j, 0), key], PAIR_SEED_LEN)
                    .try_into()
                    .expect("PAIR_SEED_LEN")
            })
            .collect();
        let commitment = commit(index, &u);
        let dealer = Self {
            index,
            parties,
            a_seed,
            u,
            coefficients,
            seeds,
            commitments: None,
        };
        Ok((dealer, commitment))
    }

    /// Ends round one: every party's commitment, indexed by party.
    ///
    /// # Errors
    ///
    /// The wrong number of commitments, a slot for this party that is not its
    /// own commitment, or a second call.
    pub fn commitments_received(&mut self, commitments: Vec<Commitment>) -> Result<()> {
        if self.commitments.is_some() {
            return Err(refuse("round one has already ended"));
        }
        if commitments.len() != self.parties {
            return Err(refuse("round one needs one commitment per party"));
        }
        if commitments[self.index] != commit(self.index, &self.u) {
            return Err(refuse("the commitment in this party's slot is not its own"));
        }
        self.commitments = Some(commitments);
        Ok(())
    }

    fn committed(&self) -> Result<&[Commitment]> {
        self.commitments
            .as_deref()
            .ok_or(refuse("nothing is revealed before every commitment is in"))
    }

    /// Round two's public reveal of `u_p`.
    ///
    /// # Errors
    ///
    /// Before [`Dealer::commitments_received`].
    pub fn reveal(&self) -> Result<Reveal> {
        self.committed()?;
        Ok(Reveal(self.u.clone()))
    }

    /// Round two's private message for party `to`: its share of this piece,
    /// and the pairwise seed `seed[index][to]`.
    ///
    /// # Errors
    ///
    /// Before [`Dealer::commitments_received`], or `to` outside the roster.
    pub fn private_share(&self, to: usize) -> Result<PrivateShare> {
        self.committed()?;
        let seed = *self.seeds.get(to).ok_or(refuse("no such party"))?;
        Ok(PrivateShare {
            share: evaluate(&self.coefficients, to as u64 + 1),
            seed,
        })
    }

    /// Finishes the ceremony from every party's reveal and private share
    /// (indexed by party), yielding this party's key share and the group key.
    ///
    /// # Errors
    ///
    /// Before [`Dealer::commitments_received`]; a missing contribution; or a
    /// reveal that does not open its commitment.
    pub fn finish(
        self,
        reveals: &[Reveal],
        received: &[PrivateShare],
    ) -> Result<(KeyShare, VerifyingKey)> {
        let commitments = self.committed()?;
        let n = self.parties;
        if reveals.len() != n || received.len() != n {
            return Err(refuse("a party's contribution is missing"));
        }
        for (p, (c, r)) in commitments.iter().zip(reveals).enumerate() {
            if commit(p, &r.0) != *c {
                return Err(refuse("a party's reveal does not open its commitment"));
            }
        }
        let u_sum = reveals
            .iter()
            .skip(1)
            .fold(reveals[0].0.clone(), |acc, r| ring::vec_add(&acc, &r.0, Q));
        let t = u_sum.iter().map(|p| ring::rshift(p, NU_T, Q_T)).collect();
        let share = received
            .iter()
            .skip(1)
            .fold(received[0].share.clone(), |acc, r| {
                ring::vec_add(&acc, &r.share, Q)
            });
        let seeds = (0..n).map(|j| (self.seeds[j], received[j].seed)).collect();
        Ok((
            KeyShare::new(self.index, share, seeds),
            VerifyingKey {
                a_seed: self.a_seed,
                t,
            },
        ))
    }
}
