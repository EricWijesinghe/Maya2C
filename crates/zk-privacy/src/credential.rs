//! Selective disclosure: proving a claim without saying what it is, or who you
//! are.
//!
//! ## Three statements, conjoined
//!
//! > I know a claim value `v` and a blinding factor `r` such that
//! > `H(subject, schema, v, r)` is a leaf of the tree committed to by
//! > `issuer_root`; **and** `v` satisfies the public predicate; **and** the bit
//! > at my credential index is zero in the bitmap committed to by
//! > `revocation_root`.
//!
//! Public: the two roots and the predicate. Private: the subject, the value,
//! the blinding factor, the index, and both authentication paths. A verifier
//! learns that *somebody* holds a live credential from that issuer satisfying
//! that predicate, and nothing else — not who, not the value, not which
//! credential.
//!
//! ## Why the issuer's signature is not in here
//!
//! It is the obvious design and it does not fit. Maya2C signatures are hybrid
//! pairs, and verifying ML-DSA-65 in R1CS means an NTT over a 23-bit prime,
//! 256-coefficient polynomials, a 6×5 matrix and SHAKE256 — on the order of
//! 10^7 to 10^8 constraints, against a joinsplit circuit that is a few tens of
//! thousands. Groth16 setup at that size is not something anyone ships. BBS+ is
//! the industry answer to exactly this and is pairing-based, so it is not
//! post-quantum and defeats the premise.
//!
//! So the chain carries that half. An issuer anchors its root in a transaction,
//! consensus verifies the hybrid signature **natively**, and the root becomes
//! consensus state. This circuit then only has to prove membership under a root
//! the chain already vouches for.
//!
//! **This proof is Groth16 over BLS12-381 and is not post-quantum.** The
//! attestation is; the presentation is not. A quantum adversary could forge a
//! presentation without being able to forge an attestation. Same standing
//! caveat as the shielded pool, and it is written here so nobody has to infer
//! it from a dependency list.
//!
//! ## Why the revocation bit is in here at all
//!
//! Because leaving it out would undo the rest. A holder who proves `age >= 18`
//! in zero knowledge and is then asked for credential index 4,721 has linked
//! the presentation to a credential — and across two verifiers, to itself. The
//! chain still publishes the bitmap for uses where the index is not sensitive;
//! this is the path for uses where it is.
//!
//! ## The predicate is public and coarse on purpose
//!
//! `AtLeast(18)` rather than the age. A predicate fine enough to identify — a
//! birth date to the day, an income to the pound — would leak through the
//! public input even though the value never does. The verifier chooses what to
//! ask; the circuit makes sure asking is all they get.

use ark_bls12_381::Fr;
use ark_ff::{AdditiveGroup, One, PrimeField};
use ark_r1cs_std::alloc::AllocVar;
use ark_r1cs_std::boolean::Boolean;
use ark_r1cs_std::eq::EqGadget;
use ark_r1cs_std::fields::FieldVar;
use ark_r1cs_std::fields::fp::FpVar;
use ark_r1cs_std::select::CondSelectGadget;
use ark_relations::gr1cs::{ConstraintSynthesizer, ConstraintSystemRef, SynthesisError};
use ark_std::rand::SeedableRng;

use crate::error::{Result, ZkError};
use crate::hash::{hash, hash_var};
use crate::params::TREE_DEPTH;
use crate::tree::{MerklePath, merkle_path};

/// Constraint synthesis returns arkworks' error, not this crate's.
type SynthResult<T> = std::result::Result<T, SynthesisError>;

/// Domain for a credential leaf.
const DOMAIN_CREDENTIAL: u64 = 0x43_52_45_44; // "CRED"

/// Domain for a revocation-bitmap leaf.
const DOMAIN_REVOCATION: u64 = 0x52_45_56_4b; // "REVK"

/// Bits in a claim value.
///
/// Sixty-four, the same width the joinsplit uses for a note value and for the
/// same reason: the range check is what stops a prover choosing a value that
/// wraps the field modulus and satisfies a comparison it should fail.
const VALUE_BITS: usize = 64;

/// What a verifier is asking.
///
/// Deliberately three shapes and no more. Each is public, so each is a thing
/// the verifier learns; a predicate language rich enough to express a birth
/// date would leak one through the public input even though the value never
/// leaves the circuit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Predicate {
    /// The value is at least this. `age >= 18`.
    AtLeast(u64),
    /// The value is at most this.
    AtMost(u64),
    /// The value is exactly this. A country code, an accreditation flag.
    EqualTo(u64),
}

impl Predicate {
    /// The public input this predicate contributes: a tag and a bound.
    #[must_use]
    pub fn to_field_elements(self) -> [Fr; 2] {
        let (tag, bound) = match self {
            Self::AtLeast(bound) => (0u64, bound),
            Self::AtMost(bound) => (1, bound),
            Self::EqualTo(bound) => (2, bound),
        };
        [Fr::from(tag), Fr::from(bound)]
    }

    /// Whether a value satisfies it, natively.
    ///
    /// The same decision the circuit makes, in twelve characters, so a holder
    /// can find out before paying for a proof — and so the tests can assert the
    /// two agree.
    #[must_use]
    pub const fn holds(self, value: u64) -> bool {
        match self {
            Self::AtLeast(bound) => value >= bound,
            Self::AtMost(bound) => value <= bound,
            Self::EqualTo(bound) => value == bound,
        }
    }
}

/// The leaf an issuer puts in its credential tree.
///
/// `H(domain, subject, schema, value, blinding)`. The blinding factor is what
/// makes this a commitment rather than a lookup table: ages, country codes and
/// accreditation flags all come from small sets, so an unblinded hash is
/// recovered by guessing.
#[must_use]
pub fn credential_leaf(subject: Fr, schema: Fr, value: u64, blinding: Fr) -> Fr {
    hash(&[
        Fr::from(DOMAIN_CREDENTIAL),
        subject,
        schema,
        Fr::from(value),
        blinding,
    ])
}

/// The leaf a revocation bitmap puts at one index.
///
/// `H(domain, index, bit)`. A leaf per bit rather than a packed word, so a
/// holder proves one bit without revealing the sixty-three around it — which
/// would narrow the index to a word and undo most of what the circuit is for.
#[must_use]
pub fn revocation_leaf(index: u64, revoked: bool) -> Fr {
    hash(&[
        Fr::from(DOMAIN_REVOCATION),
        Fr::from(index),
        Fr::from(u64::from(revoked)),
    ])
}

/// Everything the holder knows and the verifier does not.
#[derive(Clone, Debug)]
pub struct DisclosureWitness {
    /// The subject, as a field element.
    pub subject: Fr,
    /// Which schema, as a field element.
    pub schema: Fr,
    /// The claim value.
    pub value: u64,
    /// The commitment's blinding factor.
    pub blinding: Fr,
    /// Path from the credential leaf to the issuer's root.
    pub credential_path: MerklePath,
    /// Which credential index this is.
    pub index: u64,
    /// Path from the revocation leaf to the bitmap root.
    pub revocation_path: MerklePath,
}

/// What the verifier supplies and checks against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisclosurePublic {
    /// The issuer's credential-tree root, from chain state.
    pub issuer_root: Fr,
    /// The issuer's revocation-bitmap root, from chain state.
    pub revocation_root: Fr,
    /// What is being asked.
    pub predicate: Predicate,
}

impl DisclosurePublic {
    /// The public input vector, in allocation order.
    #[must_use]
    pub fn to_field_elements(&self) -> Vec<Fr> {
        let [tag, bound] = self.predicate.to_field_elements();
        vec![self.issuer_root, self.revocation_root, tag, bound]
    }
}

/// The statement.
#[derive(Clone)]
pub struct DisclosureCircuit {
    /// Absent during setup, which needs only the shape.
    pub witness: Option<DisclosureWitness>,
    /// The public half.
    pub public: DisclosurePublic,
}

impl DisclosureCircuit {
    /// The shape, with no assignment.
    #[must_use]
    pub fn blueprint() -> Self {
        Self {
            witness: None,
            public: DisclosurePublic {
                issuer_root: Fr::ZERO,
                revocation_root: Fr::ZERO,
                // Any variant: the predicate's *tag* is a witness-free public
                // input, so the shape does not depend on which one it is.
                predicate: Predicate::AtLeast(0),
            },
        }
    }

    /// The circuit for a witness.
    #[must_use]
    pub fn new(witness: DisclosureWitness, public: DisclosurePublic) -> Self {
        Self {
            witness: Some(witness),
            public,
        }
    }
}

impl ConstraintSynthesizer<Fr> for DisclosureCircuit {
    fn generate_constraints(self, cs: ConstraintSystemRef<Fr>) -> SynthResult<()> {
        let missing = || SynthesisError::AssignmentMissing;
        let witness = self.witness;

        let issuer_root = FpVar::new_input(cs.clone(), || Ok(self.public.issuer_root))?;
        let revocation_root = FpVar::new_input(cs.clone(), || Ok(self.public.revocation_root))?;
        let (tag_value, bound_value) = match self.public.predicate {
            Predicate::AtLeast(bound) => (0u64, bound),
            Predicate::AtMost(bound) => (1, bound),
            Predicate::EqualTo(bound) => (2, bound),
        };
        let tag = FpVar::new_input(cs.clone(), || Ok(Fr::from(tag_value)))?;
        let bound_var = FpVar::new_input(cs.clone(), || Ok(Fr::from(bound_value)))?;

        let subject = FpVar::new_witness(cs.clone(), || {
            Ok(witness.as_ref().ok_or_else(missing)?.subject)
        })?;
        let schema = FpVar::new_witness(cs.clone(), || {
            Ok(witness.as_ref().ok_or_else(missing)?.schema)
        })?;
        let blinding = FpVar::new_witness(cs.clone(), || {
            Ok(witness.as_ref().ok_or_else(missing)?.blinding)
        })?;

        // The value is range-checked by construction: it is built from exactly
        // VALUE_BITS witnessed bits, so a prover cannot supply one that wraps
        // the modulus and satisfies a comparison it should fail.
        let value_bits = witness_bits(cs.clone(), witness.as_ref().map(|w| w.value))?;
        let value = from_bits(&value_bits);

        // --- 1. The credential is in the issuer's tree. ---
        let leaf = hash_var(
            cs.clone(),
            &[
                FpVar::constant(Fr::from(DOMAIN_CREDENTIAL)),
                subject,
                schema,
                value.clone(),
                blinding,
            ],
        )?;
        let credential_index = fold_to_root(
            cs.clone(),
            leaf,
            witness.as_ref().map(|w| &w.credential_path),
            &issuer_root,
        )?;

        // --- 2. The revocation bit at this index is zero. ---
        //
        // The leaf is built with the bit *hard-coded to zero* rather than
        // witnessed. A witnessed bit would let a prover claim zero for a leaf
        // whose real value is one; with the constant, the only leaf that
        // authenticates is the unrevoked one, and a revoked credential simply
        // has no path that reaches the root.
        let revocation_index_var = FpVar::new_witness(cs.clone(), || {
            Ok(Fr::from(witness.as_ref().ok_or_else(missing)?.index))
        })?;
        let unrevoked = hash_var(
            cs.clone(),
            &[
                FpVar::constant(Fr::from(DOMAIN_REVOCATION)),
                revocation_index_var.clone(),
                FpVar::zero(),
            ],
        )?;
        let bitmap_index = fold_to_root(
            cs.clone(),
            unrevoked,
            witness.as_ref().map(|w| &w.revocation_path),
            &revocation_root,
        )?;

        // The two paths must be about the same credential. Without this a
        // holder could prove membership of credential A and non-revocation of
        // credential B, which is a revoked credential presenting cleanly.
        credential_index.enforce_equal(&bitmap_index)?;
        revocation_index_var.enforce_equal(&bitmap_index)?;

        // --- 3. The predicate holds. ---
        let bound_bits = witness_bits(cs.clone(), Some(bound_value))?;
        bound_var.enforce_equal(&from_bits(&bound_bits))?;

        let value_ge_bound = at_least(&value_bits, &bound_bits)?;
        let value_le_bound = at_least(&bound_bits, &value_bits)?;
        let equal = value.is_eq(&bound_var)?;

        // Selected by the public tag, so one circuit serves all three and the
        // verifying key does not depend on which is asked.
        let is_at_least = tag.is_eq(&FpVar::zero())?;
        let is_at_most = tag.is_eq(&FpVar::one())?;
        let is_equal = tag.is_eq(&FpVar::constant(Fr::from(2u64)))?;

        // Exactly one arm, so a tag outside 0..=2 satisfies nothing.
        let selected =
            Boolean::kary_or(&[is_at_least.clone(), is_at_most.clone(), is_equal.clone()])?;
        selected.enforce_equal(&Boolean::TRUE)?;

        let satisfied = Boolean::kary_or(&[
            Boolean::kary_and(&[is_at_least, value_ge_bound])?,
            Boolean::kary_and(&[is_at_most, value_le_bound])?,
            Boolean::kary_and(&[is_equal, equal])?,
        ])?;
        satisfied.enforce_equal(&Boolean::TRUE)?;

        Ok(())
    }
}

/// Witnessed bits of a `u64`, little-endian.
///
/// Named apart from the locals it produces: a function shadowed by its own
/// result is a compile error at the second call and a puzzle at the first.
fn witness_bits(cs: ConstraintSystemRef<Fr>, value: Option<u64>) -> SynthResult<Vec<Boolean<Fr>>> {
    (0..VALUE_BITS)
        .map(|index| {
            Boolean::new_witness(cs.clone(), || {
                let value = value.ok_or(SynthesisError::AssignmentMissing)?;
                Ok(value >> index & 1 == 1)
            })
        })
        .collect()
}

/// The field element those bits spell.
fn from_bits(bits: &[Boolean<Fr>]) -> FpVar<Fr> {
    let mut acc = FpVar::<Fr>::zero();
    let mut weight = Fr::one();
    for bit in bits {
        acc += FpVar::from(bit.clone()) * FpVar::constant(weight);
        weight.double_in_place();
    }
    acc
}

/// Whether `left >= right`, comparing bit vectors from the top down.
///
/// Hand-rolled because `FpVar` has no comparison gadget in `ark-r1cs-std` 0.6
/// and `UInt64` would mean a second representation of the same value — one
/// hashed, one compared, with nothing forcing them equal. These are the *same*
/// bits that build the value that goes into the leaf.
///
/// The answer is decided by the **most significant** differing bit: above it
/// the two are equal, below it nothing can change the order. So the walk runs
/// from the least significant bit upward and each difference overwrites the
/// last, leaving the highest one — which is the whole trick, and getting the
/// direction wrong is a comparator that answers with the *lowest* differing bit
/// instead. That happened: `37 >= 38` came back true, because bit 0 differs and
/// 37 has a one there.
///
/// Equal vectors leave the initial `TRUE`, which is right: `x >= x`.
fn at_least(left: &[Boolean<Fr>], right: &[Boolean<Fr>]) -> SynthResult<Boolean<Fr>> {
    let mut result = Boolean::TRUE;
    for (high, low) in left.iter().zip(right) {
        let differs = high.is_neq(low)?;
        result = Boolean::conditionally_select(&differs, high, &result)?;
    }
    Ok(result)
}

/// Folds a leaf to a root, checks it, and returns the leaf's index.
fn fold_to_root(
    cs: ConstraintSystemRef<Fr>,
    leaf: FpVar<Fr>,
    path: Option<&MerklePath>,
    root: &FpVar<Fr>,
) -> SynthResult<FpVar<Fr>> {
    let missing = || SynthesisError::AssignmentMissing;

    let mut node = leaf;
    let mut index = FpVar::<Fr>::zero();
    let mut place = Fr::one();
    for level in 0..TREE_DEPTH {
        let sibling = FpVar::new_witness(cs.clone(), || {
            path.ok_or_else(missing)?
                .siblings
                .get(level)
                .copied()
                .ok_or_else(missing)
        })?;
        let goes_right = Boolean::new_witness(cs.clone(), || {
            Ok(path.ok_or_else(missing)?.index >> level & 1 == 1)
        })?;

        index += FpVar::from(goes_right.clone()) * FpVar::constant(place);
        place.double_in_place();

        let left = FpVar::conditionally_select(&goes_right, &sibling, &node)?;
        let right = FpVar::conditionally_select(&goes_right, &node, &sibling)?;
        node = hash_var(cs.clone(), &[left, right])?;
    }
    node.enforce_equal(root)?;
    Ok(index)
}

/// Generates this circuit's keys.
///
/// A **separate** setup from the joinsplit's and the sanctions list's: a
/// Groth16 key commits to the constraint system, so a proof under the wrong one
/// does not verify — a failure that reads as "invalid proof" rather than "wrong
/// key". Keeping them apart by type is cheaper than diagnosing that.
#[must_use]
pub fn setup() -> (
    ark_groth16::ProvingKey<ark_bls12_381::Bls12_381>,
    ark_groth16::VerifyingKey<ark_bls12_381::Bls12_381>,
) {
    use ark_snark::SNARK;
    let mut rng =
        ark_std::rand::rngs::StdRng::seed_from_u64(crate::params::SETUP_SEED ^ DOMAIN_CREDENTIAL);
    ark_groth16::Groth16::<ark_bls12_381::Bls12_381>::circuit_specific_setup(
        DisclosureCircuit::blueprint(),
        &mut rng,
    )
    .expect("the disclosure circuit has a fixed shape, so setup cannot fail")
}

/// The cached proving key.
pub fn proving_key() -> &'static ark_groth16::ProvingKey<ark_bls12_381::Bls12_381> {
    static KEY: std::sync::OnceLock<ark_groth16::ProvingKey<ark_bls12_381::Bls12_381>> =
        std::sync::OnceLock::new();
    KEY.get_or_init(|| setup().0)
}

/// The cached verifying key, preprocessed.
pub fn verifying_key() -> &'static ark_groth16::PreparedVerifyingKey<ark_bls12_381::Bls12_381> {
    use ark_snark::SNARK;
    static KEY: std::sync::OnceLock<ark_groth16::PreparedVerifyingKey<ark_bls12_381::Bls12_381>> =
        std::sync::OnceLock::new();
    KEY.get_or_init(|| {
        ark_groth16::Groth16::<ark_bls12_381::Bls12_381>::process_vk(&setup().1)
            .expect("vk preprocesses")
    })
}

/// Proves a disclosure.
///
/// # Errors
///
/// Returns [`ZkError::Unsatisfiable`] if the witness does not satisfy the
/// statement. Synthesized first rather than handed straight to
/// `Groth16::prove`, which asserts satisfiability internally and would abort
/// the process instead of returning.
pub fn prove(
    witness: &DisclosureWitness,
    public: &DisclosurePublic,
) -> Result<[u8; crate::prove::PROOF_BYTES]> {
    use ark_relations::gr1cs::ConstraintSystem;
    use ark_snark::SNARK;

    let cs = ConstraintSystem::<Fr>::new_ref();
    DisclosureCircuit::new(witness.clone(), *public)
        .generate_constraints(cs.clone())
        .map_err(|error| ZkError::Prove(error.to_string()))?;
    if !cs
        .is_satisfied()
        .map_err(|error| ZkError::Prove(error.to_string()))?
    {
        return Err(ZkError::Unsatisfiable);
    }

    let mut rng = ark_std::rand::rngs::StdRng::seed_from_u64(crate::params::SETUP_SEED ^ 0x1d3a);
    let proof = ark_groth16::Groth16::<ark_bls12_381::Bls12_381>::prove(
        proving_key(),
        DisclosureCircuit::new(witness.clone(), *public),
        &mut rng,
    )
    .map_err(|error| ZkError::Prove(error.to_string()))?;

    let mut bytes = Vec::with_capacity(crate::prove::PROOF_BYTES);
    ark_serialize::CanonicalSerialize::serialize_compressed(&proof, &mut bytes)
        .map_err(|error| ZkError::Prove(error.to_string()))?;
    bytes
        .try_into()
        .map_err(|_| ZkError::Prove("proof is not the expected length".into()))
}

/// Verifies a disclosure.
///
/// # Errors
///
/// Returns [`ZkError::Malformed`] if the proof does not decode. A well-formed
/// proof that does not satisfy the statement returns `Ok(false)`.
pub fn verify(proof: &[u8; crate::prove::PROOF_BYTES], public: &DisclosurePublic) -> Result<bool> {
    use ark_snark::SNARK;
    let proof = crate::prove::decode_proof(proof)?;
    ark_groth16::Groth16::<ark_bls12_381::Bls12_381>::verify_with_processed_vk(
        verifying_key(),
        &public.to_field_elements(),
        &proof,
    )
    .map_err(|error| ZkError::Malformed(error.to_string()))
}

/// Builds the two paths a holder needs, from the issuer's full leaf sets.
///
/// # Errors
///
/// Propagates a path failure, and returns [`ZkError::UnknownLeaf`] if the
/// credential index is past either set.
pub fn witness_for(
    subject: Fr,
    schema: Fr,
    value: u64,
    blinding: Fr,
    index: u64,
    credential_leaves: &[Fr],
    revocation_leaves: &[Fr],
) -> Result<DisclosureWitness> {
    Ok(DisclosureWitness {
        subject,
        schema,
        value,
        blinding,
        credential_path: merkle_path(credential_leaves, index)?,
        index,
        revocation_path: merkle_path(revocation_leaves, index)?,
    })
}

/// The field element a 32-byte identifier becomes.
///
/// Truncated to 248 bits so it always fits: BLS12-381's scalar field is just
/// under 2^255, and a 256-bit value would reduce modulo the order — two
/// distinct subjects colliding onto one field element, which is one subject's
/// credential proving another's claim.
#[must_use]
pub fn field_from_bytes(bytes: &[u8; 32]) -> Fr {
    let mut truncated = *bytes;
    truncated[31] &= 0x1f;
    Fr::from_le_bytes_mod_order(&truncated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_relations::gr1cs::ConstraintSystem;

    const SUBJECT: u64 = 42;
    const SCHEMA: u64 = 7;
    const AGE: u64 = 34;
    const INDEX: u64 = 3;

    /// An issuer's two trees: credentials, and one revocation leaf per index.
    struct Issuer {
        credentials: Vec<Fr>,
        revocations: Vec<Fr>,
    }

    fn tree(revoked: &[u64]) -> Issuer {
        Issuer {
            credentials: (0..8u64)
                .map(|index| {
                    credential_leaf(
                        Fr::from(SUBJECT + index),
                        Fr::from(SCHEMA),
                        AGE + index,
                        Fr::from(1_000 + index),
                    )
                })
                .collect(),
            revocations: (0..8u64)
                .map(|index| revocation_leaf(index, revoked.contains(&index)))
                .collect(),
        }
    }

    fn roots(issuer: &Issuer) -> (Fr, Fr) {
        (
            merkle_path(&issuer.credentials, 0)
                .expect("path")
                .compute_root(issuer.credentials[0])
                .expect("root"),
            merkle_path(&issuer.revocations, 0)
                .expect("path")
                .compute_root(issuer.revocations[0])
                .expect("root"),
        )
    }

    fn witness(issuer: &Issuer, index: u64) -> DisclosureWitness {
        witness_for(
            Fr::from(SUBJECT + index),
            Fr::from(SCHEMA),
            AGE + index,
            Fr::from(1_000 + index),
            index,
            &issuer.credentials,
            &issuer.revocations,
        )
        .expect("witness")
    }

    fn public(issuer: &Issuer, predicate: Predicate) -> DisclosurePublic {
        let (issuer_root, revocation_root) = roots(issuer);
        DisclosurePublic {
            issuer_root,
            revocation_root,
            predicate,
        }
    }

    fn satisfied(witness: DisclosureWitness, public: DisclosurePublic) -> bool {
        let cs = ConstraintSystem::<Fr>::new_ref();
        DisclosureCircuit::new(witness, public)
            .generate_constraints(cs.clone())
            .expect("synthesize");
        cs.is_satisfied().expect("satisfiable check")
    }

    #[test]
    fn a_live_credential_satisfying_the_predicate_proves_and_verifies() {
        let issuer = tree(&[]);
        let public = public(&issuer, Predicate::AtLeast(18));
        let witness = witness(&issuer, INDEX);
        assert!(satisfied(witness.clone(), public));

        let proof = prove(&witness, &public).expect("prove");
        assert!(verify(&proof, &public).expect("verify"));
    }

    #[test]
    fn every_predicate_shape_agrees_with_its_native_twin() {
        // The circuit and `Predicate::holds` must decide the same way, or a
        // holder finds out they cannot prove something only after paying for
        // the proof.
        let issuer = tree(&[]);
        let value = AGE + INDEX;
        for predicate in [
            Predicate::AtLeast(18),
            Predicate::AtLeast(value),
            Predicate::AtLeast(value + 1),
            Predicate::AtMost(value),
            Predicate::AtMost(value - 1),
            Predicate::EqualTo(value),
            Predicate::EqualTo(value + 1),
        ] {
            assert_eq!(
                satisfied(witness(&issuer, INDEX), public(&issuer, predicate)),
                predicate.holds(value),
                "{predicate:?} disagreed with the circuit"
            );
        }
    }

    #[test]
    fn a_revoked_credential_cannot_prove() {
        // The lie is consistent: the credential is genuinely in the issuer's
        // tree, the predicate genuinely holds, and the holder genuinely owns
        // it. Only the revocation bit differs, so only that guard can refuse.
        let issuer = tree(&[INDEX]);
        let public = public(&issuer, Predicate::AtLeast(18));
        assert!(
            !satisfied(witness(&issuer, INDEX), public),
            "a revoked credential presented cleanly"
        );

        // An unrevoked sibling of the same issuer still proves, so the refusal
        // is about the credential rather than about the tree.
        assert!(satisfied(witness(&issuer, INDEX + 1), public));
    }

    #[test]
    fn a_holder_cannot_pair_one_credential_with_anothers_revocation_bit() {
        // The attack the index equality exists for: prove membership of a
        // revoked credential while proving non-revocation of a live one. Both
        // paths here are genuine and both reach their real roots.
        let issuer = tree(&[INDEX]);
        let public = public(&issuer, Predicate::AtLeast(18));

        let mut mixed = witness(&issuer, INDEX);
        let clean = witness(&issuer, INDEX + 1);
        mixed.revocation_path = clean.revocation_path;
        mixed.index = INDEX + 1;

        assert!(
            !satisfied(mixed, public),
            "a revoked credential borrowed a live credential's bit"
        );
    }

    #[test]
    fn a_credential_from_another_issuer_does_not_prove() {
        let mine = tree(&[]);
        let theirs = Issuer {
            credentials: (0..8u64)
                .map(|index| {
                    credential_leaf(Fr::from(999u64), Fr::from(SCHEMA), 99, Fr::from(index))
                })
                .collect(),
            revocations: mine.revocations.clone(),
        };
        // The witness is genuine for `theirs`; the public root is `mine`.
        assert!(!satisfied(
            witness(&theirs, INDEX),
            public(&mine, Predicate::AtLeast(18))
        ));
    }

    #[test]
    fn changing_the_value_breaks_the_membership_proof() {
        // A holder cannot claim a value they were not issued: the value is
        // hashed into the leaf, so changing it changes the leaf and the path no
        // longer reaches the root.
        let issuer = tree(&[]);
        let mut lying = witness(&issuer, INDEX);
        lying.value = 21;
        assert!(!satisfied(lying, public(&issuer, Predicate::AtLeast(18))));
    }

    #[test]
    fn a_bound_past_the_value_is_refused_rather_than_wrapping() {
        // Without the bit decomposition a prover could supply a value that
        // reduces modulo the field order and satisfies a comparison it should
        // fail. The value is built from exactly VALUE_BITS bits.
        assert_eq!(VALUE_BITS, 64);
        let issuer = tree(&[]);
        assert!(!satisfied(
            witness(&issuer, INDEX),
            public(&issuer, Predicate::AtLeast(u64::MAX))
        ));
    }

    #[test]
    fn a_proof_does_not_carry_to_a_different_root() {
        // Otherwise reissuing a tree would not actually retire the old one.
        let issuer = tree(&[]);
        let here = public(&issuer, Predicate::AtLeast(18));
        let proof = prove(&witness(&issuer, INDEX), &here).expect("prove");

        let other = tree(&[7]);
        let elsewhere = public(&other, Predicate::AtLeast(18));
        assert_ne!(here.revocation_root, elsewhere.revocation_root);
        assert!(!verify(&proof, &elsewhere).expect("verify"));
    }

    #[test]
    fn a_proof_does_not_carry_to_a_different_predicate() {
        // The predicate is a public input, so a proof for "at least 18" must
        // not verify as "at least 65".
        let issuer = tree(&[]);
        let asked = public(&issuer, Predicate::AtLeast(18));
        let proof = prove(&witness(&issuer, INDEX), &asked).expect("prove");
        assert!(!verify(&proof, &public(&issuer, Predicate::AtLeast(65))).expect("verify"));
        assert!(!verify(&proof, &public(&issuer, Predicate::EqualTo(18))).expect("verify"));
    }

    #[test]
    fn a_predicate_tag_the_enum_cannot_produce_verifies_nothing() {
        // The `selected` constraint. Without it an unknown tag makes every arm
        // false and the conjunction vacuous — a proof of nothing verifying as
        // though it proved something.
        use ark_snark::SNARK;
        let issuer = tree(&[]);
        let asked = public(&issuer, Predicate::AtLeast(18));
        let proof = prove(&witness(&issuer, INDEX), &asked).expect("prove");

        let mut inputs = asked.to_field_elements();
        inputs[2] = Fr::from(3u64);
        let decoded = crate::prove::decode_proof(&proof).expect("decode");
        assert!(
            !ark_groth16::Groth16::<ark_bls12_381::Bls12_381>::verify_with_processed_vk(
                verifying_key(),
                &inputs,
                &decoded,
            )
            .expect("verify")
        );
    }

    #[test]
    fn the_circuit_takes_exactly_four_public_inputs() {
        // Two roots, a tag and a bound. Anything more would be a value the
        // verifier has to know, and the point is that it knows only the roots
        // and what it asked.
        let issuer = tree(&[]);
        let cs = ConstraintSystem::<Fr>::new_ref();
        DisclosureCircuit::new(
            witness(&issuer, INDEX),
            public(&issuer, Predicate::AtLeast(18)),
        )
        .generate_constraints(cs.clone())
        .expect("synthesize");
        // arkworks counts the constant 1 alongside the declared inputs.
        assert_eq!(cs.num_instance_variables(), 5);
    }

    #[test]
    fn two_subjects_never_collide_onto_one_field_element() {
        // A 256-bit identifier reduced modulo the field order would let one
        // subject's credential prove another's claim.
        let mut seen = std::collections::HashSet::new();
        for byte in 0..=255u8 {
            let mut bytes = [byte; 32];
            bytes[0] = byte;
            assert!(
                seen.insert(field_from_bytes(&bytes).to_string()),
                "collision at {byte}"
            );
        }
    }
}
