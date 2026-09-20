//! The joinsplit circuit.
//!
//! One statement covers every shielded operation. A joinsplit consumes
//! [`JOINSPLIT_INPUTS`] notes and creates [`JOINSPLIT_OUTPUTS`] notes, with two
//! public value terms bridging the transparent side:
//!
//! | Operation | `public_in` | `public_out` |
//! |---|---|---|
//! | Shield (mint into the pool) | amount | 0 |
//! | Shielded transfer | 0 | 0 |
//! | Unshield (withdraw) | 0 | amount |
//!
//! Fixed arity is what makes the operations indistinguishable. A spend with one
//! real input pads with a zero-value dummy, and nothing on chain says which is
//! which.
//!
//! # What the circuit proves
//!
//! For each input note: that the prover knows a spending key whose address owns
//! the note, that the note's commitment sits in the tree under the public
//! anchor, and that the published nullifier is the one that note yields. For
//! each output: that the published commitment opens to a note of the claimed
//! value. Across all four: that value is conserved.
//!
//! # What it deliberately does not prove
//!
//! That `public_in` and `public_out` are within 64 bits. Those arrive from the
//! transaction's wire format, where they are already `u64`, and the verifier
//! supplies them — a prover cannot choose them independently of what the node
//! reads off the wire. The *private* note values do get an explicit range
//! check, because a prover chooses those freely and unconstrained values would
//! let a sum wrap the field modulus and fake conservation.

use ark_bls12_381::Fr;
use ark_ff::{AdditiveGroup, One};
use ark_r1cs_std::alloc::AllocVar;
use ark_r1cs_std::boolean::Boolean;
use ark_r1cs_std::eq::EqGadget;
use ark_r1cs_std::fields::FieldVar;
use ark_r1cs_std::fields::fp::FpVar;
use ark_r1cs_std::select::CondSelectGadget;
use ark_relations::gr1cs::{ConstraintSynthesizer, ConstraintSystemRef, SynthesisError};

use crate::field::bytes_to_limbs;
use crate::hash::hash_var;
use crate::note::{Note, SpendingKey};
use crate::params::{
    DOMAIN_ADDRESS, DOMAIN_COMMITMENT, DOMAIN_NULLIFIER, JOINSPLIT_INPUTS, JOINSPLIT_OUTPUTS,
    TREE_DEPTH,
};
use crate::tree::MerklePath;

/// Bits in a note value.
const VALUE_BITS: usize = 64;

/// Everything a verifier sees.
///
/// The field order here *is* the public input order the circuit allocates in.
/// They are produced by one function and consumed by another, so a reordering
/// would break verification rather than silently mis-bind an input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JoinSplitPublic {
    /// Tree root the input notes are proved against.
    pub anchor: Fr,
    /// Nullifiers retired by this joinsplit.
    pub nullifiers: [Fr; JOINSPLIT_INPUTS],
    /// Commitments created by this joinsplit.
    pub commitments: [Fr; JOINSPLIT_OUTPUTS],
    /// Transparent value entering the pool.
    pub public_in: u64,
    /// Transparent value leaving the pool.
    pub public_out: u64,
    /// Fee paid to the miner, in the clear.
    pub fee: u64,
    /// Transparent recipient of `public_out`.
    ///
    /// Bound into the proof so a miner cannot redirect a withdrawal: changing
    /// it changes the public input vector and the pairing check fails.
    pub recipient: [u8; 32],
}

impl JoinSplitPublic {
    /// Flattens to the public input vector, in allocation order.
    #[must_use]
    pub fn to_field_elements(&self) -> Vec<Fr> {
        let recipient = bytes_to_limbs(&self.recipient);
        vec![
            self.anchor,
            self.nullifiers[0],
            self.nullifiers[1],
            self.commitments[0],
            self.commitments[1],
            Fr::from(self.public_in),
            Fr::from(self.public_out),
            Fr::from(self.fee),
            recipient[0],
            recipient[1],
        ]
    }
}

/// A note being spent.
#[derive(Clone, Debug)]
pub struct SpendWitness {
    /// The note itself.
    pub note: Note,
    /// Key authorizing the spend.
    pub key: SpendingKey,
    /// Path proving the note's commitment is under the anchor.
    pub path: MerklePath,
    /// Whether this is a zero-value pad rather than a real note.
    ///
    /// A dummy still publishes a nullifier — derived from its random `rho`, so
    /// it collides with nothing — but its Merkle path is not checked, because
    /// there is no tree entry to check it against.
    pub is_dummy: bool,
}

/// A note being created.
#[derive(Clone, Debug)]
pub struct OutputWitness {
    /// The new note.
    pub note: Note,
}

/// The full private input to a joinsplit.
#[derive(Clone, Debug)]
pub struct JoinSplitWitness {
    /// Notes consumed.
    pub inputs: [SpendWitness; JOINSPLIT_INPUTS],
    /// Notes created.
    pub outputs: [OutputWitness; JOINSPLIT_OUTPUTS],
    /// Root the inputs are proved against.
    pub anchor: Fr,
    /// Transparent value entering.
    pub public_in: u64,
    /// Transparent value leaving.
    pub public_out: u64,
    /// Fee.
    pub fee: u64,
    /// Transparent recipient of `public_out`.
    pub recipient: [u8; 32],
}

impl JoinSplitWitness {
    /// Derives the public inputs this witness implies.
    ///
    /// Deriving rather than accepting them separately removes a whole class of
    /// mistake: the prover cannot publish a nullifier or commitment that does
    /// not match the notes it actually used.
    #[must_use]
    pub fn public(&self) -> JoinSplitPublic {
        JoinSplitPublic {
            anchor: self.anchor,
            nullifiers: [
                self.inputs[0].note.nullifier(&self.inputs[0].key),
                self.inputs[1].note.nullifier(&self.inputs[1].key),
            ],
            commitments: [
                self.outputs[0].note.commitment(),
                self.outputs[1].note.commitment(),
            ],
            public_in: self.public_in,
            public_out: self.public_out,
            fee: self.fee,
            recipient: self.recipient,
        }
    }

    /// Total value on the input side.
    #[must_use]
    pub fn input_total(&self) -> u128 {
        u128::from(self.public_in)
            + self
                .inputs
                .iter()
                .map(|input| u128::from(input.note.value))
                .sum::<u128>()
    }

    /// Total value on the output side, including the fee.
    #[must_use]
    pub fn output_total(&self) -> u128 {
        u128::from(self.public_out)
            + u128::from(self.fee)
            + self
                .outputs
                .iter()
                .map(|output| u128::from(output.note.value))
                .sum::<u128>()
    }
}

/// The joinsplit statement.
#[derive(Clone)]
pub struct JoinSplitCircuit {
    /// Absent during setup, which needs only the circuit's shape.
    pub witness: Option<JoinSplitWitness>,
}

impl JoinSplitCircuit {
    /// A shape-only instance, for parameter generation.
    #[must_use]
    pub fn blueprint() -> Self {
        Self { witness: None }
    }

    /// A provable instance.
    #[must_use]
    pub fn new(witness: JoinSplitWitness) -> Self {
        Self {
            witness: Some(witness),
        }
    }
}

/// Witnesses a `u64` as 64 bits and rebuilds it as a field element.
///
/// Constructing the value from exactly 64 bits *is* the range check: no
/// assignment can represent something larger, so a prover cannot pick values
/// that wrap the modulus and forge value conservation.
fn witness_value(
    cs: ConstraintSystemRef<Fr>,
    value: Option<u64>,
) -> Result<FpVar<Fr>, SynthesisError> {
    let mut acc = FpVar::<Fr>::zero();
    let mut weight = Fr::one();

    for index in 0..VALUE_BITS {
        let bit = Boolean::new_witness(cs.clone(), || {
            let value = value.ok_or(SynthesisError::AssignmentMissing)?;
            Ok((value >> index) & 1 == 1)
        })?;
        acc += FpVar::from(bit) * FpVar::constant(weight);
        weight.double_in_place();
    }

    Ok(acc)
}

impl ConstraintSynthesizer<Fr> for JoinSplitCircuit {
    fn generate_constraints(self, cs: ConstraintSystemRef<Fr>) -> Result<(), SynthesisError> {
        let witness = self.witness;
        let missing = || SynthesisError::AssignmentMissing;

        // --- Public inputs, in the order `JoinSplitPublic` flattens them. ---

        let anchor = FpVar::new_input(cs.clone(), || {
            Ok(witness.as_ref().ok_or_else(missing)?.anchor)
        })?;

        let mut public_nullifiers = Vec::with_capacity(JOINSPLIT_INPUTS);
        for index in 0..JOINSPLIT_INPUTS {
            public_nullifiers.push(FpVar::new_input(cs.clone(), || {
                Ok(witness.as_ref().ok_or_else(missing)?.public().nullifiers[index])
            })?);
        }

        let mut public_commitments = Vec::with_capacity(JOINSPLIT_OUTPUTS);
        for index in 0..JOINSPLIT_OUTPUTS {
            public_commitments.push(FpVar::new_input(cs.clone(), || {
                Ok(witness.as_ref().ok_or_else(missing)?.public().commitments[index])
            })?);
        }

        let public_in = FpVar::new_input(cs.clone(), || {
            Ok(Fr::from(witness.as_ref().ok_or_else(missing)?.public_in))
        })?;
        let public_out = FpVar::new_input(cs.clone(), || {
            Ok(Fr::from(witness.as_ref().ok_or_else(missing)?.public_out))
        })?;
        let fee = FpVar::new_input(cs.clone(), || {
            Ok(Fr::from(witness.as_ref().ok_or_else(missing)?.fee))
        })?;

        // The transparent recipient rides along as two limbs. Nothing
        // constrains them; being public inputs is what binds them to the proof.
        for limb in 0..2usize {
            let _ = FpVar::new_input(cs.clone(), || {
                let witness = witness.as_ref().ok_or_else(missing)?;
                Ok(bytes_to_limbs(&witness.recipient)[limb])
            })?;
        }

        // --- Input notes. ---

        let mut input_sum = FpVar::<Fr>::zero();

        for (index, public_nullifier) in public_nullifiers.iter().enumerate() {
            let input = |witness: &JoinSplitWitness| witness.inputs[index].clone();

            let value = witness_value(cs.clone(), witness.as_ref().map(|w| input(w).note.value))?;

            let key = FpVar::new_witness(cs.clone(), || {
                Ok(input(witness.as_ref().ok_or_else(missing)?).key.0)
            })?;
            let rho = FpVar::new_witness(cs.clone(), || {
                Ok(input(witness.as_ref().ok_or_else(missing)?).note.rho)
            })?;
            let rand = FpVar::new_witness(cs.clone(), || {
                Ok(input(witness.as_ref().ok_or_else(missing)?).note.rand)
            })?;
            let is_dummy = Boolean::new_witness(cs.clone(), || {
                Ok(input(witness.as_ref().ok_or_else(missing)?).is_dummy)
            })?;

            // The address is derived, not witnessed: that is what proves the
            // prover holds the key that owns the note, rather than merely
            // knowing which address it belongs to.
            let address = hash_var(
                cs.clone(),
                &[FpVar::constant(Fr::from(DOMAIN_ADDRESS)), key.clone()],
            )?;

            let commitment = hash_var(
                cs.clone(),
                &[
                    FpVar::constant(Fr::from(DOMAIN_COMMITMENT)),
                    value.clone(),
                    address,
                    rho.clone(),
                    rand,
                ],
            )?;

            // A dummy carries no value, so it cannot conjure funds from a note
            // that was never in the tree.
            (&value * FpVar::from(is_dummy.clone())).enforce_equal(&FpVar::zero())?;

            // Fold the commitment up to a root.
            let mut node = commitment;
            for level in 0..TREE_DEPTH {
                let sibling = FpVar::new_witness(cs.clone(), || {
                    let witness = witness.as_ref().ok_or_else(missing)?;
                    input(witness)
                        .path
                        .siblings
                        .get(level)
                        .copied()
                        .ok_or_else(missing)
                })?;
                let goes_right = Boolean::new_witness(cs.clone(), || {
                    let witness = witness.as_ref().ok_or_else(missing)?;
                    Ok((input(witness).path.index >> level) & 1 == 1)
                })?;

                let left = FpVar::conditionally_select(&goes_right, &sibling, &node)?;
                let right = FpVar::conditionally_select(&goes_right, &node, &sibling)?;
                node = hash_var(cs.clone(), &[left, right])?;
            }

            // Only real notes must be in the tree.
            node.conditional_enforce_equal(&anchor, &!&is_dummy)?;

            let nullifier = hash_var(
                cs.clone(),
                &[FpVar::constant(Fr::from(DOMAIN_NULLIFIER)), key, rho],
            )?;
            nullifier.enforce_equal(public_nullifier)?;

            input_sum += value;
        }

        // --- Output notes. ---

        let mut output_sum = FpVar::<Fr>::zero();

        for (index, public_commitment) in public_commitments.iter().enumerate() {
            let output = |witness: &JoinSplitWitness| witness.outputs[index].clone();

            let value = witness_value(cs.clone(), witness.as_ref().map(|w| output(w).note.value))?;

            let address = FpVar::new_witness(cs.clone(), || {
                Ok(output(witness.as_ref().ok_or_else(missing)?).note.address.0)
            })?;
            let rho = FpVar::new_witness(cs.clone(), || {
                Ok(output(witness.as_ref().ok_or_else(missing)?).note.rho)
            })?;
            let rand = FpVar::new_witness(cs.clone(), || {
                Ok(output(witness.as_ref().ok_or_else(missing)?).note.rand)
            })?;

            let commitment = hash_var(
                cs.clone(),
                &[
                    FpVar::constant(Fr::from(DOMAIN_COMMITMENT)),
                    value.clone(),
                    address,
                    rho,
                    rand,
                ],
            )?;
            commitment.enforce_equal(public_commitment)?;

            output_sum += value;
        }

        // --- Value conservation. ---
        //
        // Every term is at most 64 bits and there are at most six of them, so
        // the sums stay far below the field modulus and cannot wrap.
        (input_sum + public_in).enforce_equal(&(output_sum + public_out + fee))?;

        Ok(())
    }
}
