//! Groth16 setup, proving, and verification.
//!
//! # The trusted setup problem
//!
//! Groth16 needs structured reference strings generated from secret randomness.
//! Whoever knows that randomness — the "toxic waste" — can produce proofs of
//! false statements. Here, that means minting shielded coins out of nothing,
//! and because a shielded pool hides values, no supply audit would ever notice.
//!
//! This module generates the parameters deterministically from
//! [`crate::params::SETUP_SEED`]. That makes them reproducible,
//! which is exactly what a test suite needs and exactly what a real deployment
//! must never have: anyone can re-run the setup and recover the toxic waste.
//!
//! [`SETUP_IS_TRUSTED`] is `false` for that reason, and the node refuses to
//! serve a network that claims to hold real value while this is so. Replacing
//! this with the output of a multi-party ceremony — where at least one honest
//! participant destroys their contribution — is what would make the pool sound.
//!
//! # Why the key is pinned
//!
//! The verifying key is consensus. A different key rejects every proof the
//! network has already accepted. [`PINNED_VERIFYING_KEY_HASH`] fails the build's
//! tests if a dependency bump, a constraint reordering, or a parameter change
//! alters it, so the hard fork is caught here rather than in production.

use ark_bls12_381::{Bls12_381, Fr};
use ark_groth16::{Groth16, PreparedVerifyingKey, Proof, ProvingKey, VerifyingKey};
use ark_relations::gr1cs::{ConstraintSynthesizer, ConstraintSystem};
use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};
use ark_snark::SNARK;
use ark_std::UniformRand;
use ark_std::rand::rngs::StdRng;
use ark_std::rand::{RngCore, SeedableRng};
use std::sync::OnceLock;

use crate::circuit::{JoinSplitCircuit, JoinSplitPublic, JoinSplitWitness};
use crate::error::{Result, ZkError};
use crate::params::SETUP_SEED;

/// Size of a compressed Groth16 proof over BLS12-381.
pub const PROOF_BYTES: usize = 192;

/// Whether these parameters came from a ceremony nobody could have subverted.
///
/// Always `false` while the setup is seeded deterministically. Consensus code
/// should refuse to run a value-bearing network when this is `false`.
pub const SETUP_IS_TRUSTED: bool = false;

/// Whether the shielded parameters can be relied on to secure real value.
///
/// A function rather than a bare constant read so that consensus code has a
/// single call site to gate on, and so the check cannot be constant-folded out
/// of a test that exists to catch someone flipping it.
#[must_use]
pub fn setup_is_trusted() -> bool {
    SETUP_IS_TRUSTED
}

/// BLAKE3 of the compressed verifying key.
///
/// Regenerate with the `pinned_verifying_key_hash_is_current` test, and treat
/// any change as a hard fork.
pub const PINNED_VERIFYING_KEY_HASH: [u8; 32] =
    hex_literal(b"1522679f65bc1ab37764562e9227229e90748c16834d25d121a8a98c2521bc1d");

/// Parses a 64-character hex literal at compile time.
const fn hex_literal(input: &[u8; 64]) -> [u8; 32] {
    const fn nibble(byte: u8) -> u8 {
        match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            _ => panic!("verifying key hash must be lowercase hex"),
        }
    }
    let mut out = [0u8; 32];
    let mut index = 0;
    while index < 32 {
        out[index] = (nibble(input[index * 2]) << 4) | nibble(input[index * 2 + 1]);
        index += 1;
    }
    out
}

/// Generates the proving and verifying keys.
///
/// Deterministic: the same seed yields the same keys on every machine, which is
/// what lets the verifying key be pinned by hash.
#[must_use]
pub fn setup() -> (ProvingKey<Bls12_381>, VerifyingKey<Bls12_381>) {
    let mut rng = StdRng::seed_from_u64(SETUP_SEED);
    Groth16::<Bls12_381>::circuit_specific_setup(JoinSplitCircuit::blueprint(), &mut rng)
        .expect("the joinsplit circuit has a fixed shape, so setup cannot fail")
}

/// The cached proving key.
///
/// Only provers need this. It is large and takes a moment to build, so it is
/// generated once on first use rather than shipped as a binary blob.
pub fn proving_key() -> &'static ProvingKey<Bls12_381> {
    static KEY: OnceLock<ProvingKey<Bls12_381>> = OnceLock::new();
    KEY.get_or_init(|| setup().0)
}

/// The cached verifying key, preprocessed for fast pairing checks.
pub fn verifying_key() -> &'static PreparedVerifyingKey<Bls12_381> {
    static KEY: OnceLock<PreparedVerifyingKey<Bls12_381>> = OnceLock::new();
    KEY.get_or_init(|| Groth16::<Bls12_381>::process_vk(&setup().1).expect("vk preprocesses"))
}

/// BLAKE3 of a verifying key's canonical encoding.
#[must_use]
pub fn verifying_key_hash(key: &VerifyingKey<Bls12_381>) -> [u8; 32] {
    let mut bytes = Vec::new();
    key.serialize_compressed(&mut bytes)
        .expect("a verifying key serializes");
    *blake3::hash(&bytes).as_bytes()
}

/// Checks a verifying key against the pinned hash.
///
/// # Errors
///
/// Returns [`ZkError::VerifyingKeyMismatch`] when the key is not the one
/// consensus expects.
pub fn check_pinned(key: &VerifyingKey<Bls12_381>) -> Result<()> {
    let actual = verifying_key_hash(key);
    if actual == PINNED_VERIFYING_KEY_HASH {
        return Ok(());
    }
    Err(ZkError::VerifyingKeyMismatch {
        expected: hex::encode(PINNED_VERIFYING_KEY_HASH),
        actual: hex::encode(actual),
    })
}

/// Samples a uniform field element, for note blinding factors.
pub fn random_scalar<R: RngCore>(rng: &mut R) -> Fr {
    Fr::rand(rng)
}

/// Proves a joinsplit.
///
/// Checks value conservation natively first. The circuit enforces it too, but
/// an unsatisfiable constraint system surfaces as an opaque synthesis error,
/// and a caller that got the arithmetic wrong deserves to be told which way.
///
/// # Errors
///
/// Returns [`ZkError::ValueImbalance`] when the sums do not match, or
/// [`ZkError::Prove`] if proof generation fails.
pub fn prove(witness: &JoinSplitWitness) -> Result<[u8; PROOF_BYTES]> {
    let inputs = witness.input_total();
    let outputs = witness.output_total();
    if inputs != outputs {
        return Err(ZkError::ValueImbalance { inputs, outputs });
    }

    // ark-groth16 asserts satisfiability inside `prove` rather than returning
    // an error, so an unprovable witness would abort the process. Synthesizing
    // first turns the most common caller mistake — a Merkle path that does not
    // match the anchor, usually because the wallet's view of the tree is stale
    // — into a `Result` the caller can act on.
    let cs = ConstraintSystem::<Fr>::new_ref();
    JoinSplitCircuit::new(witness.clone())
        .generate_constraints(cs.clone())
        .map_err(|error| ZkError::Prove(error.to_string()))?;
    if !cs
        .is_satisfied()
        .map_err(|error| ZkError::Prove(error.to_string()))?
    {
        return Err(ZkError::Unsatisfiable);
    }

    let mut rng = StdRng::seed_from_u64(rand_seed());
    let proof = Groth16::<Bls12_381>::prove(
        proving_key(),
        JoinSplitCircuit::new(witness.clone()),
        &mut rng,
    )
    .map_err(|error| ZkError::Prove(error.to_string()))?;

    encode_proof(&proof)
}

/// Entropy for proof randomization.
///
/// Groth16 proofs are randomized; reusing randomness across two proofs of the
/// same statement would make them linkable, which in a shielded pool leaks
/// exactly what the pool exists to hide.
fn rand_seed() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    // A monotonic counter alongside the clock, so two proofs produced within
    // the same clock tick still get distinct randomness.
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0);
    nanos
        ^ COUNTER
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

/// Serializes a proof to its compressed form.
fn encode_proof(proof: &Proof<Bls12_381>) -> Result<[u8; PROOF_BYTES]> {
    let mut bytes = Vec::with_capacity(PROOF_BYTES);
    proof
        .serialize_compressed(&mut bytes)
        .map_err(|error| ZkError::Prove(error.to_string()))?;
    bytes
        .try_into()
        .map_err(|_| ZkError::Prove("proof was not the expected width".to_string()))
}

/// Parses a compressed proof.
///
/// Uses the validating decoder: it rejects points off the curve or outside the
/// prime-order subgroup. The unchecked variant would be faster and would also
/// hand an attacker a soundness break.
///
/// # Errors
///
/// Returns [`ZkError::Malformed`] for anything that is not a valid proof.
pub fn decode_proof(bytes: &[u8; PROOF_BYTES]) -> Result<Proof<Bls12_381>> {
    Proof::<Bls12_381>::deserialize_compressed(&bytes[..])
        .map_err(|error| ZkError::Malformed(error.to_string()))
}

/// Verifies a proof against its public inputs.
///
/// # Errors
///
/// Returns [`ZkError::Malformed`] if the proof does not decode. A well-formed
/// proof that simply does not satisfy the statement returns `Ok(false)`.
pub fn verify(proof: &[u8; PROOF_BYTES], public: &JoinSplitPublic) -> Result<bool> {
    let proof = decode_proof(proof)?;
    Groth16::<Bls12_381>::verify_with_processed_vk(
        verifying_key(),
        &public.to_field_elements(),
        &proof,
    )
    .map_err(|error| ZkError::Malformed(error.to_string()))
}
