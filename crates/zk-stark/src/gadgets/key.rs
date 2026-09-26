//! Knowledge of a hash-based signing key's secret, bound to a message.
//!
//! The key pair is `pk = compress(sk, KEY)`. The proof shows knowledge of an
//! `sk` hashing to the public `pk`, and the message is a public value: the
//! verifier's Fiat–Shamir transcript absorbs every public value before it
//! draws a challenge, so a proof made for one message fails for another. That
//! makes the proof a signature of knowledge — post-quantum, because it rests
//! only on Poseidon2 preimage resistance.
//!
//! This is the key-ownership piece of the shielded pool: a note's owner
//! proves they hold the `sk` behind the `pk` committed in the note.

use super::poseidon::{self, PermAir};
use super::{Domain, SecretDigest, domain};
use crate::hash::{DIGEST, Digest, F, compress};
use crate::{Proof, ZkError};
use p3_air::{Air, AirBuilder, BaseAir, WindowAccess as _};
use p3_matrix::dense::RowMajorMatrix;

const ROWS: usize = 2;

/// `pk = compress(sk, KEY)`.
pub fn public_key(sk: &Digest) -> Digest {
    compress(sk, &domain(Domain::Key))
}

/// The key-knowledge AIR. Public values: `pk ‖ message`.
pub struct KeyAir {
    perm: PermAir,
}

impl Default for KeyAir {
    fn default() -> Self {
        Self {
            perm: poseidon::perm_air(),
        }
    }
}

impl BaseAir<F> for KeyAir {
    fn width(&self) -> usize {
        poseidon::width_with(0)
    }

    fn num_public_values(&self) -> usize {
        2 * DIGEST
    }
}

impl<AB: AirBuilder<F = F>> Air<AB> for KeyAir {
    fn eval(&self, builder: &mut AB) {
        poseidon::eval_permutation(&self.perm, builder);
        let main = builder.main();
        let row = main.current_slice();
        let inputs = poseidon::inputs(row);
        let digest = poseidon::digest(row);
        let public: Vec<AB::Expr> = builder.public_values().iter().map(|&v| v.into()).collect();
        let tag = domain(Domain::Key);

        let mut first = builder.when_first_row();
        for j in 0..DIGEST {
            first.assert_eq(inputs[DIGEST + j], AB::Expr::from(tag[j]));
            first.assert_eq(digest[j], public[j].clone());
        }
        // The message (public[8..16]) is unconstrained on purpose: it is bound
        // by the transcript, not by an equation.
    }
}

/// The trace for `sk`, without any check.
#[must_use]
pub fn trace(sk: &Digest) -> RowMajorMatrix<F> {
    super::assemble(&[super::state(sk, &domain(Domain::Key))], &[], 0, ROWS)
}

fn public(pk: &Digest, message: &Digest) -> Vec<F> {
    pk.iter().chain(message).copied().collect()
}

/// Proves knowledge of `sk`, bound to `message`. Returns the proof and `pk`.
///
/// # Errors
///
/// [`ZkError::Entropy`] if blinding randomness cannot be drawn.
pub fn prove(sk: &SecretDigest, message: &Digest) -> Result<(Proof, Digest), ZkError> {
    let sk = sk.to_field();
    let pk = public_key(&sk);
    let proof = crate::prove(&KeyAir::default(), trace(&sk), &public(&pk, message))?;
    Ok((proof, pk))
}

/// Verifies a proof of knowledge of the secret behind `pk`, for `message`.
///
/// # Errors
///
/// [`ZkError::Rejected`] or [`ZkError::Malformed`].
pub fn verify(proof: &Proof, pk: &Digest, message: &Digest) -> Result<(), ZkError> {
    crate::verify(&KeyAir::default(), proof, &public(pk, message))
}
