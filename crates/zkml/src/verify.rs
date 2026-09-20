//! Checking a proof without running the model.
//!
//! This is the half that runs inside consensus, so it is written for a hostile
//! caller: every buffer is bounded before it is parsed, the verifying key must
//! declare the one circuit size the SRS supports, a proof with trailing bytes
//! is refused, and the verification strategy is the one with no randomness.
//!
//! # `SingleStrategy`, never `AccumulatorStrategy`
//!
//! halo2's accumulator strategy scales its MSM by a scalar drawn from `OsRng`.
//! For a valid proof the answer is the same either way; but a verifier whose
//! code path draws randomness is one whose behaviour on a *crafted* input is a
//! probability rather than a fact, and a consensus rule cannot be a
//! probability. `SingleStrategy` draws nothing.
//!
//! # The exact public vector is bound, length included
//!
//! The circuit constrains instance rows `0..=n` and nothing after. What stops a
//! caller appending a second "class" — junk, or a zero that leaves the instance
//! polynomial unchanged — is that `VerifierSHPLONK` sets `QUERY_INSTANCE =
//! false`, so halo2 absorbs every public value into the Fiat–Shamir transcript
//! individually. A different vector is a different transcript. That is a
//! property of the verifier *choice*: a commitment-querying verifier would bind
//! the polynomial instead, and trailing zeros would verify. Two tests in
//! `tests/proof_tests.rs` pin it.
//!
//! # A panic in the verifier is a refusal, not a crash
//!
//! The key and proof parsers are halo2's, not this crate's, and nothing
//! guarantees they cannot be made to panic by bytes nobody anticipated. A panic
//! in a host function would unwind out of wasmtime and take the node with it —
//! a one-transaction way to stop every validator. [`verify`] catches it and
//! reports a malformed key instead.

use std::panic::{AssertUnwindSafe, catch_unwind};

use halo2_axiom::SerdeFormat;
use halo2_axiom::halo2curves::bn256::{Bn256, G1Affine};
use halo2_axiom::plonk::{VerifyingKey, verify_proof};
use halo2_axiom::poly::kzg::commitment::KZGCommitmentScheme;
use halo2_axiom::poly::kzg::multiopen::VerifierSHPLONK;
use halo2_axiom::poly::kzg::strategy::SingleStrategy;
use halo2_axiom::transcript::{Blake2bRead, Challenge255, TranscriptReadBuffer};

use crate::circuit::{MlpCircuit, fr};
use crate::error::{Result, ZkmlError};
use crate::model::MAX_INPUTS;
use crate::srs::{K, params};

/// Largest verifying key accepted.
pub const MAX_VK_BYTES: usize = 16 * 1024;

/// Largest proof accepted.
pub const MAX_PROOF_BYTES: usize = 16 * 1024;

/// Most public inputs: the widest input layer, plus the class.
pub const MAX_PUBLIC_INPUTS: usize = MAX_INPUTS + 1;

/// Length of a model identifier.
pub const MODEL_ID_LEN: usize = 32;

/// Domain string for model identifiers.
const MODEL_ID_DOMAIN: &[u8] = b"maya2c.zkml.model-id.v1";

/// How keys are serialised: compressed points, with every field element
/// checked against the modulus on the way in. `RawBytesUnchecked` would skip
/// the on-curve check, which for bytes from a transaction is not an option.
pub const FORMAT: SerdeFormat = SerdeFormat::Processed;

/// Version byte halo2 writes at the start of a verifying key.
const VK_VERSION: u8 = 0x02;

/// A model's identity: a hash of its verifying key.
///
/// The weights are fixed columns, so they are committed in the key, so the key
/// *is* the model. Anyone holding the ONNX file can recompute this with
/// `prove::keygen` and check that a contract's registered model is the one it
/// claims to be.
#[must_use]
pub fn model_id(vk: &[u8]) -> [u8; MODEL_ID_LEN] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(MODEL_ID_DOMAIN);
    hasher.update(vk);
    *hasher.finalize().as_bytes()
}

/// Verifies that the model with key `vk` maps `public[..n]` to class
/// `public[n]`.
///
/// Returns `Ok(false)` for a well-formed proof that does not verify, including
/// one that does not parse — a bad proof is an answer, not a malfunction.
///
/// # Errors
///
/// - [`ZkmlError::Oversized`] for any buffer past its bound.
/// - [`ZkmlError::MalformedKey`] for a key that does not parse, declares a
///   different circuit size, carries trailing bytes, or makes halo2 panic.
pub fn verify(vk: &[u8], public: &[i64], proof: &[u8]) -> Result<bool> {
    bounded("verifying key", vk.len(), MAX_VK_BYTES)?;
    bounded("proof", proof.len(), MAX_PROOF_BYTES)?;
    bounded("public inputs", public.len(), MAX_PUBLIC_INPUTS)?;

    catch_unwind(AssertUnwindSafe(|| verify_unguarded(vk, public, proof)))
        .unwrap_or(Err(ZkmlError::MalformedKey("the verifier panicked")))
}

fn bounded(what: &'static str, len: usize, max: usize) -> Result<()> {
    if len > max {
        return Err(ZkmlError::Oversized { what, len, max });
    }
    Ok(())
}

fn verify_unguarded(vk_bytes: &[u8], public: &[i64], proof: &[u8]) -> Result<bool> {
    // The header is checked by hand before halo2 sees it. `VerifyingKey::read`
    // builds an evaluation domain of `2^k` from the `k` it finds here, so a key
    // declaring `k = 30` is a request to allocate gigabytes.
    if vk_bytes.len() < 6 || vk_bytes[0] != VK_VERSION {
        return Err(ZkmlError::MalformedKey("unexpected version"));
    }
    if u32::from_le_bytes([vk_bytes[1], vk_bytes[2], vk_bytes[3], vk_bytes[4]]) != K {
        return Err(ZkmlError::MalformedKey(
            "circuit size is not the one the SRS supports",
        ));
    }

    let mut reader = vk_bytes;
    let vk = VerifyingKey::<G1Affine>::read::<_, MlpCircuit>(&mut reader, FORMAT, ())
        .map_err(|_| ZkmlError::MalformedKey("does not parse"))?;
    // Trailing bytes would give one model two identities.
    if !reader.is_empty() {
        return Err(ZkmlError::MalformedKey("trailing bytes"));
    }

    let instance: Vec<_> = public.iter().copied().map(fr).collect();
    let srs = params();
    let mut cursor = proof;
    let verified = {
        let mut transcript = Blake2bRead::<_, G1Affine, Challenge255<G1Affine>>::init(&mut cursor);
        verify_proof::<
            KZGCommitmentScheme<Bn256>,
            VerifierSHPLONK<'_, Bn256>,
            Challenge255<G1Affine>,
            _,
            SingleStrategy<'_, Bn256>,
        >(
            srs,
            &vk,
            SingleStrategy::new(srs),
            &[&[&instance]],
            &mut transcript,
        )
        .is_ok()
    };

    // A proof followed by junk verifies exactly as the proof alone would. It is
    // refused anyway: a transaction id covers its input bytes, so tolerated
    // padding is a free way to mint distinct transactions carrying one proof.
    Ok(verified && cursor.is_empty())
}
