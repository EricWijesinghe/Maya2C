//! Proving, verifying, and measuring what a proof is worth.

use p3_air::symbolic::{AirLayout, SymbolicAirBuilder};
use p3_air::{Air, BaseAir};
use p3_matrix::dense::RowMajorMatrix;
use p3_security::fri::FriRegime;
use p3_uni_stark::{
    ConjecturedSecurity, ProvenSecurity, QuotientAir, StarkSecurityParams, VerifierConstraintFolder,
};

use crate::ZkError;
use crate::config::{self, Challenge, Config, LOG_BLOWUP, NUM_QUERIES, QUERY_POW_BITS};
use crate::hash::F;

/// Every AIR this crate proves: usable by the prover, the verifier and the
/// symbolic analysis the security estimate needs.
pub trait StarkAir:
    BaseAir<F>
    + QuotientAir<Config>
    + Air<SymbolicAirBuilder<F>>
    + Air<SymbolicAirBuilder<F, Challenge>>
    + for<'a> Air<VerifierConstraintFolder<'a, Config>>
    + for<'a> Air<p3_air::DebugConstraintBuilder<'a, F>>
{
}

impl<A> StarkAir for A where
    A: BaseAir<F>
        + QuotientAir<Config>
        + Air<SymbolicAirBuilder<F>>
        + Air<SymbolicAirBuilder<F, Challenge>>
        + for<'a> Air<VerifierConstraintFolder<'a, Config>>
        + for<'a> Air<p3_air::DebugConstraintBuilder<'a, F>>
{
}

/// A serialized proof.
#[derive(Clone, PartialEq, Eq)]
pub struct Proof {
    bytes: Vec<u8>,
}

impl core::fmt::Debug for Proof {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Proof({} bytes)", self.bytes.len())
    }
}

impl Proof {
    /// The encoding.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Wraps bytes received from elsewhere; decoding happens in [`verify`].
    #[must_use]
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }
}

/// Proves that `trace` satisfies `air` with `public` values.
///
/// Callers check the witness natively *before* building a trace (every
/// statement in [`crate::pool`] and [`crate::gadgets`] does), because
/// Plonky3's prover asserts rather than reports an unsatisfied constraint.
///
/// # Errors
///
/// [`ZkError::Entropy`] if blinding randomness cannot be drawn.
pub fn prove<A: StarkAir>(
    air: &A,
    trace: RowMajorMatrix<F>,
    public: &[F],
) -> Result<Proof, ZkError> {
    let config = config::config()?;
    let proof = p3_uni_stark::prove(&config, air, trace, public);
    let bytes = postcard::to_allocvec(&proof).map_err(|e| ZkError::Malformed(e.to_string()))?;
    Ok(Proof { bytes })
}

/// Verifies `proof` against `air` and `public`.
///
/// # Errors
///
/// [`ZkError::Malformed`] if the bytes do not decode, [`ZkError::Rejected`]
/// if the proof does not verify.
pub fn verify<A: StarkAir>(air: &A, proof: &Proof, public: &[F]) -> Result<(), ZkError> {
    let config = config::config()?;
    let decoded: p3_uni_stark::Proof<Config> =
        postcard::from_bytes(&proof.bytes).map_err(|e| ZkError::Malformed(e.to_string()))?;
    p3_uni_stark::verify(&config, air, &decoded, public)
        .map_err(|e| ZkError::Rejected(format!("{e:?}")))
}

/// Conjectured and proven security, in bits, of `air` proved over
/// `2^degree_bits` rows, from Plonky3's own estimator.
#[must_use]
pub fn security_bits<A: StarkAir>(air: &A, degree_bits: usize) -> (usize, usize) {
    let fri = FriRegime {
        log_blowup: LOG_BLOWUP,
        num_queries: NUM_QUERIES,
        log_final_poly_len: 0,
        max_log_arity: 1,
        commit_pow_bits: 0,
        query_pow_bits: QUERY_POW_BITS,
    };
    // The extension field has ~124 bits; Keccak-256 collision resistance is 128.
    let params = StarkSecurityParams::from_air::<F, Challenge, A>(
        fri,
        air,
        AirLayout::from_air::<F>(air),
        124,
        128,
        2,
    );
    let conjectured = ConjecturedSecurity::compute_from_params(&params, degree_bits).security_bits;
    let proven = ProvenSecurity::compute_from_proof(degree_bits, &params).security_bits();
    (conjectured, proven)
}

/// The number of rows a proof covers, `log2`, from its bytes.
///
/// # Errors
///
/// [`ZkError::Malformed`] if the bytes do not decode.
pub fn degree_bits(proof: &Proof) -> Result<usize, ZkError> {
    let decoded: p3_uni_stark::Proof<Config> =
        postcard::from_bytes(&proof.bytes).map_err(|e| ZkError::Malformed(e.to_string()))?;
    Ok(decoded.degree_bits)
}
