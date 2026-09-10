//! Key generation and proving. Off-chain only.
//!
//! In its own crate, which nothing the node links depends on. A validator
//! verifies; it has no reason to link a prover, and a prover in the node is a
//! prover somebody eventually calls from a consensus path.

use halo2_axiom::halo2curves::bn256::{Bn256, G1Affine};
use halo2_axiom::plonk::{ProvingKey, create_proof, keygen_pk, keygen_vk};
use halo2_axiom::poly::kzg::commitment::KZGCommitmentScheme;
use halo2_axiom::poly::kzg::multiopen::ProverSHPLONK;
use halo2_axiom::transcript::{Blake2bWrite, Challenge255, TranscriptWriterBuffer};
use rand::rngs::OsRng;

use maya_zkml::circuit::{MlpCircuit, Witness};
use maya_zkml::error::{Result, ZkmlError};
use maya_zkml::model::QuantizedMlp;
use maya_zkml::srs::params;
use maya_zkml::verify::{FORMAT, MODEL_ID_LEN, model_id};

/// Everything needed to prove statements about one model.
pub struct ProvingSetup {
    model: QuantizedMlp,
    pk: ProvingKey<G1Affine>,
    vk: Vec<u8>,
}

impl core::fmt::Debug for ProvingSetup {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ProvingSetup")
            .field("model_id", &hex32(&self.model_id()))
            .field("vk_bytes", &self.vk.len())
            .finish()
    }
}

impl ProvingSetup {
    /// The serialised verifying key — what a contract stores and passes to the
    /// host function.
    #[must_use]
    pub fn verifying_key(&self) -> &[u8] {
        &self.vk
    }

    /// The model's identity, [`model_id`] of the key.
    #[must_use]
    pub fn model_id(&self) -> [u8; MODEL_ID_LEN] {
        model_id(&self.vk)
    }

    /// The model this setup proves statements about.
    #[must_use]
    pub fn model(&self) -> &QuantizedMlp {
        &self.model
    }
}

/// Generates the keys for a model.
///
/// Deterministic: the same model and the same SRS always produce the same
/// verifying key, byte for byte, which is what lets anyone holding the ONNX
/// file recompute a contract's model id and check it.
///
/// # Errors
///
/// [`ZkmlError::Proving`] if the model does not fit the circuit size, which
/// the bounds in [`maya_zkml::model`] are meant to rule out.
pub fn keygen(model: &QuantizedMlp) -> Result<ProvingSetup> {
    let circuit = MlpCircuit::for_keygen(model.clone());
    let srs = params();
    let vk = keygen_vk(srs, &circuit).map_err(|e| ZkmlError::Proving(e.to_string()))?;
    let bytes = vk.to_bytes(FORMAT);
    let pk = keygen_pk(srs, vk, &circuit).map_err(|e| ZkmlError::Proving(e.to_string()))?;
    Ok(ProvingSetup {
        model: model.clone(),
        pk,
        vk: bytes,
    })
}

/// Proves the model classifies `input` as the class it computes.
///
/// # Errors
///
/// [`ZkmlError::WrongInputWidth`] for an input of the wrong width, and
/// [`ZkmlError::Proving`] if proving fails.
pub fn prove(setup: &ProvingSetup, input: &[i8]) -> Result<(Vec<u8>, usize)> {
    let evaluation = setup.model.evaluate(input)?;
    let wide: Vec<i64> = input.iter().map(|&v| i64::from(v)).collect();
    let witness = Witness::honest(&setup.model, &wide);
    let proof = prove_witness(setup, witness)?;
    Ok((proof, evaluation.class))
}

/// Proves an arbitrary witness — honest or not.
///
/// For tests: a lying witness must either fail to prove or produce a proof the
/// verifier rejects. Which of the two happens is halo2's business; the tests
/// only care that no path ends in `verify == true`.
///
/// # Errors
///
/// [`ZkmlError::Proving`] if halo2 refuses to prove it.
pub fn prove_witness(setup: &ProvingSetup, witness: Witness) -> Result<Vec<u8>> {
    let instance = witness.public_inputs();
    let circuit = MlpCircuit::with_witness(setup.model.clone(), witness);
    let srs = params();

    let mut transcript = Blake2bWrite::<_, G1Affine, Challenge255<_>>::init(vec![]);
    create_proof::<KZGCommitmentScheme<Bn256>, ProverSHPLONK<'_, Bn256>, _, _, _, _>(
        srs,
        &setup.pk,
        &[circuit],
        &[&[&instance]],
        OsRng,
        &mut transcript,
    )
    .map_err(|e| ZkmlError::Proving(e.to_string()))?;
    Ok(transcript.finalize())
}

fn hex32(bytes: &[u8; 32]) -> String {
    bytes[..8].iter().map(|b| format!("{b:02x}")).collect()
}
