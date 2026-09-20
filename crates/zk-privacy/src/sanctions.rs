//! Proving that an account is **not** on a list, without saying which account.
//!
//! ## The problem this solves
//!
//! A compliance check and anonymity look like opposites. The check wants to
//! know the party is not sanctioned; anonymity wants nobody to learn who the
//! party is. Revealing the identifier satisfies the first and destroys the
//! second, and a bridge that did it for every payment would leak the whole
//! customer base of every counterparty to every observer of the chain.
//!
//! So the statement proved here is the negative one:
//!
//! > I know a 32-byte identifier `x` that is **not** in the list committed to
//! > by `root`.
//!
//! `root` is public. `x` is not. A verifier learns that a payment's party
//! cleared the list and nothing else — not who they are, not which entry
//! bracketed them, not even how many parties the prover has checked.
//!
//! ## Why non-membership needs a *sorted* tree
//!
//! Membership is easy: show a path. Non-membership has no path to show —
//! absence leaves no leaf. The construction is to sort the list and prove a
//! *gap*: exhibit two adjacent leaves `lo` and `hi` with
//!
//! ```text
//! lo < x < hi   and   lo, hi are adjacent in the tree
//! ```
//!
//! Adjacency is what makes it a proof rather than a claim. Any two listed
//! entries with `x` between them prove nothing — the list could hold `x`
//! somewhere in the middle. Only neighbours, at indices `i` and `i + 1` of a
//! sorted list, leave no room for `x` to hide.
//!
//! The two sentinels in [`SanctionsList::build`] are what make every `x`
//! bracketable, including one below every entry or above every entry.
//!
//! ## Why identifiers are compared as bytes, not as field elements
//!
//! `FpVar` has no comparison gadget in `ark-r1cs-std` 0.6 — ordering field
//! elements means bit-decomposing them and hand-rolling a comparator, which is
//! the kind of code that is subtly wrong in one corner. `UInt8` does have one,
//! and `[T]: CmpGadget` gives lexicographic ordering over a slice of them for
//! free. So an identifier is 32 bytes big-endian in the circuit, ordered
//! lexicographically, and that ordering is exactly `<[u8; 32]>::cmp` natively —
//! one definition, two places, no drift to reason about.
//!
//! ## What this does not prove
//!
//! That `x` is the identifier of the party in the payment. Binding a proof to a
//! payment is the caller's job and is done by the payment carrying a commitment
//! to `x` that this proof is checked against — otherwise a prover proves
//! non-membership of an identifier nobody is paying.
//!
//! It also does not prove the list is the right list. `root` is a public input,
//! so a verifier that does not already know which root it expects learns
//! nothing from a proof against a root the prover chose.

use ark_bls12_381::Fr;
use ark_ff::AdditiveGroup;
use ark_r1cs_std::alloc::AllocVar;
use ark_r1cs_std::boolean::Boolean;
use ark_r1cs_std::cmp::CmpGadget;
use ark_r1cs_std::eq::EqGadget;
use ark_r1cs_std::fields::FieldVar;
use ark_r1cs_std::fields::fp::FpVar;
use ark_r1cs_std::select::CondSelectGadget;
use ark_r1cs_std::uint8::UInt8;
use ark_relations::gr1cs::{ConstraintSynthesizer, ConstraintSystemRef, SynthesisError};
use ark_std::rand::SeedableRng;

use crate::error::{Result, ZkError};
use crate::hash::{hash, hash_var};
use crate::params::TREE_DEPTH;
use crate::tree::{MerklePath, merkle_path};

/// Constraint synthesis returns arkworks' error, not this crate's.
type SynthResult<T> = std::result::Result<T, SynthesisError>;

/// Domain separator for a sanctions-list leaf.
///
/// Distinct from every domain in [`crate::circuit`], so a note commitment can
/// never be read as a list entry or the reverse.
const DOMAIN_SANCTIONS_LEAF: u64 = 0x53_41_4e_43; // "SANC"

/// Bytes in an identifier.
pub const IDENTIFIER_BYTES: usize = 32;

/// An account identifier as the list holds it.
///
/// Opaque 32 bytes: how an IBAN or a BIC becomes one is the bridge's business
/// (`maya_iso20022::sanctions`), and keeping that out of here means this crate
/// never needs to know what an IBAN is.
pub type Identifier = [u8; IDENTIFIER_BYTES];

/// The lowest sentinel: below every real identifier.
const FLOOR: Identifier = [0x00; IDENTIFIER_BYTES];

/// The highest sentinel: above every real identifier.
const CEILING: Identifier = [0xff; IDENTIFIER_BYTES];

/// The leaf a listed identifier hashes to.
#[must_use]
pub fn leaf(identifier: &Identifier) -> Fr {
    let limbs = crate::field::bytes_to_limbs(identifier);
    hash(&[Fr::from(DOMAIN_SANCTIONS_LEAF), limbs[0], limbs[1]])
}

/// A published sanctions list, sorted, with its sentinels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SanctionsList {
    /// Every entry including both sentinels, ascending, without duplicates.
    entries: Vec<Identifier>,
}

impl SanctionsList {
    /// Builds a list from arbitrary entries.
    ///
    /// Sorts, removes duplicates, and adds the two sentinels. Sorting here
    /// rather than trusting the caller is what makes the gap argument sound:
    /// an unsorted list has adjacent leaves that bracket nothing.
    ///
    /// # Errors
    ///
    /// Returns [`ZkError::UnknownLeaf`] if the list would not fit the tree, and
    /// refuses an entry equal to either sentinel — those two positions are the
    /// construction's and an entry there would make a real identifier
    /// unbracketable.
    pub fn build(entries: impl IntoIterator<Item = Identifier>) -> Result<Self> {
        let mut entries: Vec<Identifier> = entries.into_iter().collect();
        if entries
            .iter()
            .any(|entry| *entry == FLOOR || *entry == CEILING)
        {
            return Err(ZkError::LimbOutOfRange { index: 0 });
        }
        entries.push(FLOOR);
        entries.push(CEILING);
        entries.sort_unstable();
        entries.dedup();

        if entries.len() as u64 > crate::tree::TREE_CAPACITY {
            return Err(ZkError::UnknownLeaf {
                index: entries.len() as u64,
            });
        }
        Ok(Self { entries })
    }

    /// Whether the list holds this identifier.
    #[must_use]
    pub fn contains(&self, identifier: &Identifier) -> bool {
        self.entries.binary_search(identifier).is_ok()
    }

    /// The leaves, in tree order.
    #[must_use]
    pub fn leaves(&self) -> Vec<Fr> {
        self.entries.iter().map(leaf).collect()
    }

    /// The root every proof is checked against.
    ///
    /// # Errors
    ///
    /// Propagates a path failure, which for a list that fits the tree cannot
    /// happen.
    pub fn root(&self) -> Result<Fr> {
        let leaves = self.leaves();
        merkle_path(&leaves, 0)?.compute_root(leaves[0])
    }

    /// The witness proving `identifier` is absent.
    ///
    /// # Errors
    ///
    /// Returns [`ZkError::UnknownLeaf`] if the identifier **is** on the list —
    /// there is no gap to exhibit, and a witness that pretended otherwise would
    /// be a proof of a false statement.
    pub fn absence_witness(&self, identifier: &Identifier) -> Result<AbsenceWitness> {
        // `Err(index)` from a binary search on a sorted slice is the insertion
        // point: the number of entries strictly below `identifier`. So the
        // bracketing pair is `index - 1` and `index`, and both exist because
        // the sentinels sit at both ends.
        let index = match self.entries.binary_search(identifier) {
            Ok(found) => {
                return Err(ZkError::UnknownLeaf {
                    index: found as u64,
                });
            }
            Err(insertion) => insertion,
        };
        let leaves = self.leaves();
        Ok(AbsenceWitness {
            identifier: *identifier,
            low: self.entries[index - 1],
            high: self.entries[index],
            low_path: merkle_path(&leaves, (index - 1) as u64)?,
            high_path: merkle_path(&leaves, index as u64)?,
        })
    }
}

/// Everything the prover knows and the verifier does not.
#[derive(Clone, Debug)]
pub struct AbsenceWitness {
    /// The identifier being cleared.
    pub identifier: Identifier,
    /// The listed entry immediately below it.
    pub low: Identifier,
    /// The listed entry immediately above it.
    pub high: Identifier,
    /// `low`'s path to the root.
    pub low_path: MerklePath,
    /// `high`'s path to the root.
    pub high_path: MerklePath,
}

/// The statement: "the identifier I know is not under this root."
#[derive(Clone)]
pub struct AbsenceCircuit {
    /// Absent during setup, which needs only the circuit's shape.
    pub witness: Option<AbsenceWitness>,
    /// The list's root. The one public input.
    pub root: Fr,
}

impl ConstraintSynthesizer<Fr> for AbsenceCircuit {
    fn generate_constraints(self, cs: ConstraintSystemRef<Fr>) -> SynthResult<()> {
        let witness = self.witness;

        let root = FpVar::new_input(cs.clone(), || Ok(self.root))?;

        let identifier = witness_bytes(cs.clone(), witness.as_ref().map(|w| w.identifier))?;
        let low = witness_bytes(cs.clone(), witness.as_ref().map(|w| w.low))?;
        let high = witness_bytes(cs.clone(), witness.as_ref().map(|w| w.high))?;

        // The gap. Strict on both sides: `low < identifier` also rules out the
        // identifier *being* `low`, which is the case a non-strict comparison
        // would let through — and that case is precisely a sanctioned party
        // proving itself clear.
        low.as_slice()
            .is_lt(identifier.as_slice())?
            .enforce_equal(&Boolean::TRUE)?;
        identifier
            .as_slice()
            .is_lt(high.as_slice())?
            .enforce_equal(&Boolean::TRUE)?;

        // Both neighbours are in the tree.
        let low_index = fold_to_root(
            cs.clone(),
            &low,
            witness.as_ref().map(|w| &w.low_path),
            &root,
        )?;
        let high_index = fold_to_root(
            cs.clone(),
            &high,
            witness.as_ref().map(|w| &w.high_path),
            &root,
        )?;

        // And they are *adjacent*. Without this the prover picks any two listed
        // entries that happen to straddle the identifier, and the list may hold
        // the identifier between them — which is the whole statement, defeated.
        (high_index - low_index).enforce_equal(&FpVar::one())?;

        Ok(())
    }
}

/// Allocates 32 witness bytes.
fn witness_bytes(
    cs: ConstraintSystemRef<Fr>,
    value: Option<Identifier>,
) -> SynthResult<Vec<UInt8<Fr>>> {
    (0..IDENTIFIER_BYTES)
        .map(|index| {
            UInt8::new_witness(cs.clone(), || {
                value
                    .map(|bytes| bytes[index])
                    .ok_or(SynthesisError::AssignmentMissing)
            })
        })
        .collect()
}

/// Hashes an identifier to its leaf, folds it to a root, enforces equality with
/// `root`, and returns the leaf's index as a field element.
///
/// The index is returned rather than discarded because adjacency — the
/// constraint that makes this a non-membership proof at all — is a statement
/// about two indices, and reconstructing it from the path bits is the only
/// place it exists.
fn fold_to_root(
    cs: ConstraintSystemRef<Fr>,
    identifier: &[UInt8<Fr>],
    path: Option<&MerklePath>,
    root: &FpVar<Fr>,
) -> SynthResult<FpVar<Fr>> {
    let missing = || SynthesisError::AssignmentMissing;

    // The two 128-bit limbs, little-endian within each half, matching
    // `field::bytes_to_limbs` exactly. Built from the same bytes the
    // comparison used, so the value that was ordered is the value that is
    // hashed — allocating the limbs separately would let a prover order one
    // identifier and prove membership of another.
    let limb = |half: usize| -> SynthResult<FpVar<Fr>> {
        let mut value = FpVar::<Fr>::zero();
        let mut scale = Fr::from(1u64);
        for byte in &identifier[half * 16..half * 16 + 16] {
            value += byte.to_fp()? * FpVar::constant(scale);
            scale *= Fr::from(256u64);
        }
        Ok(value)
    };

    let mut node = hash_var(
        cs.clone(),
        &[
            FpVar::constant(Fr::from(DOMAIN_SANCTIONS_LEAF)),
            limb(0)?,
            limb(1)?,
        ],
    )?;

    let mut index = FpVar::<Fr>::zero();
    let mut place = Fr::from(1u64);
    for level in 0..TREE_DEPTH {
        let sibling = FpVar::new_witness(cs.clone(), || {
            path.ok_or_else(missing)?
                .siblings
                .get(level)
                .copied()
                .ok_or_else(missing)
        })?;
        let goes_right = Boolean::new_witness(cs.clone(), || {
            Ok((path.ok_or_else(missing)?.index >> level) & 1 == 1)
        })?;

        index += FpVar::from(goes_right.clone()) * FpVar::constant(place);
        place *= Fr::from(2u64);

        let left = FpVar::conditionally_select(&goes_right, &sibling, &node)?;
        let right = FpVar::conditionally_select(&goes_right, &node, &sibling)?;
        node = hash_var(cs.clone(), &[left, right])?;
    }

    node.enforce_equal(root)?;
    Ok(index)
}

/// A blueprint circuit for setup, which needs the shape and no witness.
impl AbsenceCircuit {
    /// The shape, with no assignment.
    #[must_use]
    pub fn blueprint() -> Self {
        Self {
            witness: None,
            root: Fr::ZERO,
        }
    }

    /// The circuit for a witness against a root.
    #[must_use]
    pub fn new(witness: AbsenceWitness, root: Fr) -> Self {
        Self {
            witness: Some(witness),
            root,
        }
    }
}

/// Generates this circuit's proving and verifying keys.
///
/// A **separate** setup from the joinsplit's. Two circuits cannot share a
/// Groth16 key: the key commits to the constraint system, so a proof under the
/// wrong one does not verify — which is a failure that reads as "the proof is
/// invalid" rather than "you used the wrong key", so the two are kept apart by
/// type rather than by discipline.
///
/// Deterministic from the same seed for the same reason `prove::setup` is: the
/// verifying key can then be pinned by hash.
#[must_use]
pub fn setup() -> (
    ark_groth16::ProvingKey<ark_bls12_381::Bls12_381>,
    ark_groth16::VerifyingKey<ark_bls12_381::Bls12_381>,
) {
    use ark_snark::SNARK;
    let mut rng = ark_std::rand::rngs::StdRng::seed_from_u64(
        crate::params::SETUP_SEED ^ DOMAIN_SANCTIONS_LEAF,
    );
    ark_groth16::Groth16::<ark_bls12_381::Bls12_381>::circuit_specific_setup(
        AbsenceCircuit::blueprint(),
        &mut rng,
    )
    .expect("the absence circuit has a fixed shape, so setup cannot fail")
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

/// Proves that the witness's identifier is absent from the list under `root`.
///
/// # Errors
///
/// Returns [`ZkError::Unsatisfiable`] if the witness does not satisfy the
/// statement — a witness whose neighbours are not adjacent, do not bracket the
/// identifier, or are not in the tree. Synthesized first rather than handed
/// straight to `Groth16::prove`, which asserts satisfiability internally and
/// would abort the process instead of returning.
pub fn prove(witness: &AbsenceWitness, root: Fr) -> Result<[u8; crate::prove::PROOF_BYTES]> {
    use ark_relations::gr1cs::ConstraintSystem;
    use ark_snark::SNARK;

    let cs = ConstraintSystem::<Fr>::new_ref();
    AbsenceCircuit::new(witness.clone(), root)
        .generate_constraints(cs.clone())
        .map_err(|error| ZkError::Prove(error.to_string()))?;
    if !cs
        .is_satisfied()
        .map_err(|error| ZkError::Prove(error.to_string()))?
    {
        return Err(ZkError::Unsatisfiable);
    }

    let mut rng = ark_std::rand::rngs::StdRng::seed_from_u64(crate::params::SETUP_SEED ^ 0x9e37);
    let proof = ark_groth16::Groth16::<ark_bls12_381::Bls12_381>::prove(
        proving_key(),
        AbsenceCircuit::new(witness.clone(), root),
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

/// Verifies an absence proof against a root.
///
/// # Errors
///
/// Returns [`ZkError::Malformed`] if the proof does not decode. A well-formed
/// proof that does not satisfy the statement returns `Ok(false)`.
pub fn verify(proof: &[u8; crate::prove::PROOF_BYTES], root: Fr) -> Result<bool> {
    use ark_snark::SNARK;
    let proof = crate::prove::decode_proof(proof)?;
    ark_groth16::Groth16::<ark_bls12_381::Bls12_381>::verify_with_processed_vk(
        verifying_key(),
        &[root],
        &proof,
    )
    .map_err(|error| ZkError::Malformed(error.to_string()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use ark_relations::gr1cs::ConstraintSystem;

    /// A listed identifier, spaced so there is room to construct one between
    /// any two of them.
    fn listed(n: u8) -> Identifier {
        let mut id = [0u8; IDENTIFIER_BYTES];
        id[0] = n * 16;
        id
    }

    /// An identifier strictly between `listed(n)` and `listed(n + 1)`.
    fn between(n: u8) -> Identifier {
        let mut id = listed(n);
        id[IDENTIFIER_BYTES - 1] = 1;
        id
    }

    fn list() -> SanctionsList {
        SanctionsList::build((1..=4).map(listed)).expect("build")
    }

    /// Whether the circuit accepts this witness against this root.
    fn satisfied(witness: AbsenceWitness, root: Fr) -> bool {
        let cs = ConstraintSystem::<Fr>::new_ref();
        AbsenceCircuit::new(witness, root)
            .generate_constraints(cs.clone())
            .expect("synthesize");
        cs.is_satisfied().expect("satisfiable check")
    }

    #[test]
    fn the_list_sorts_and_brackets_every_identifier() {
        let list = list();
        for n in 1..=4 {
            assert!(list.contains(&listed(n)));
            assert!(!list.contains(&between(n)));
        }
        // Below every entry and above every entry: the sentinels are what make
        // these two cases bracketable rather than special.
        let mut below = [0u8; IDENTIFIER_BYTES];
        below[IDENTIFIER_BYTES - 1] = 1;
        list.absence_witness(&below).expect("below the first entry");
        let mut above = [0xff; IDENTIFIER_BYTES];
        above[IDENTIFIER_BYTES - 1] = 0xfe;
        list.absence_witness(&above).expect("above the last entry");
    }

    #[test]
    fn an_absent_identifier_proves_and_verifies() {
        let list = list();
        let root = list.root().expect("root");
        let witness = list.absence_witness(&between(2)).expect("witness");
        assert!(satisfied(witness.clone(), root));

        let proof = prove(&witness, root).expect("prove");
        assert!(verify(&proof, root).expect("verify"));
    }

    #[test]
    fn a_listed_identifier_has_no_witness_at_all() {
        // The first line of defence: there is no gap to exhibit, so a
        // sanctioned party cannot even build the input to the prover.
        let list = list();
        assert!(list.absence_witness(&listed(3)).is_err());
    }

    #[test]
    fn a_listed_identifier_cannot_be_smuggled_past_the_gap_check() {
        // The lie is made *consistent*: every other part of the witness is
        // genuine — both neighbours are really in the tree, really adjacent,
        // and their paths really reach the root — and only the identifier is
        // one the list holds. So the bracket constraint is the only guard that
        // can refuse it, and this test fails if that guard is removed.
        let list = list();
        let root = list.root().expect("root");
        let mut witness = list.absence_witness(&between(2)).expect("witness");
        witness.identifier = witness.low;
        assert!(
            !satisfied(witness, root),
            "a `<=` bracket would let a listed party clear itself"
        );
    }

    #[test]
    fn two_listed_entries_that_are_not_neighbours_prove_nothing() {
        // The attack the adjacency constraint exists for. `listed(2)` is on the
        // list and sits between `listed(1)` and `listed(3)`, so a prover that
        // could pick any two straddling entries would clear it. Both paths here
        // are genuine and both brackets hold — only adjacency fails.
        let list = list();
        let root = list.root().expect("root");
        let leaves = list.leaves();
        let low_index = list.entries.binary_search(&listed(1)).expect("listed");
        let high_index = list.entries.binary_search(&listed(3)).expect("listed");

        let witness = AbsenceWitness {
            identifier: listed(2),
            low: listed(1),
            high: listed(3),
            low_path: merkle_path(&leaves, low_index as u64).expect("path"),
            high_path: merkle_path(&leaves, high_index as u64).expect("path"),
        };
        assert!(
            !satisfied(witness, root),
            "without adjacency, a listed party is bracketed by its own neighbours"
        );
    }

    #[test]
    fn a_neighbour_that_is_not_in_the_tree_is_refused() {
        // An invented `high` that brackets the identifier and is adjacent by
        // index, but was never listed. Consistent everywhere except the tree
        // membership this checks.
        let list = list();
        let root = list.root().expect("root");
        let mut witness = list.absence_witness(&between(2)).expect("witness");
        witness.high = listed(9);
        assert!(!satisfied(witness, root));
    }

    #[test]
    fn a_proof_against_one_root_does_not_verify_against_another() {
        let list = list();
        let root = list.root().expect("root");
        let witness = list.absence_witness(&between(2)).expect("witness");
        let proof = prove(&witness, root).expect("prove");

        // A list with one more entry: a different commitment, so a proof drawn
        // against the old one must not carry over. Otherwise adding a party to
        // the list would not actually stop them.
        let extended = SanctionsList::build((1..=5).map(listed)).expect("build");
        let other = extended.root().expect("root");
        assert_ne!(root, other);
        assert!(!verify(&proof, other).expect("verify"));
    }

    #[test]
    fn a_newly_listed_identifier_stops_proving() {
        let target = between(2);
        let before = list();
        before
            .absence_witness(&target)
            .expect("absent from the old list");

        let after = SanctionsList::build((1..=4).map(listed).chain([target])).expect("build");
        assert!(after.absence_witness(&target).is_err());
    }

    #[test]
    fn the_sentinels_cannot_be_listed() {
        // An entry at either sentinel would make every identifier past it
        // unbracketable, which is a denial of service on the honest side rather
        // than a soundness break — refused at construction so it cannot become
        // a published list nobody can prove against.
        assert!(SanctionsList::build([FLOOR]).is_err());
        assert!(SanctionsList::build([CEILING]).is_err());
    }

    #[test]
    fn the_circuit_takes_exactly_one_public_input() {
        // The root, and nothing else. A second public input would be a value
        // the verifier has to supply and therefore has to know — and the point
        // of this circuit is that the verifier knows nothing but the list.
        let list = list();
        let root = list.root().expect("root");
        let witness = list.absence_witness(&between(2)).expect("witness");
        let cs = ConstraintSystem::<Fr>::new_ref();
        AbsenceCircuit::new(witness, root)
            .generate_constraints(cs.clone())
            .expect("synthesize");
        // arkworks counts the constant 1 alongside the declared inputs.
        assert_eq!(cs.num_instance_variables(), 2);
    }
}
