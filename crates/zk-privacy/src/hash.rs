//! Poseidon hashing, native and in-circuit.
//!
//! Both forms live here deliberately. They must agree on every input for the
//! scheme to work at all — a native commitment the circuit computes differently
//! is a note nobody can ever spend — and keeping them apart is how that drift
//! happens. `native_and_circuit_agree` in the tests is the guard.

use ark_bls12_381::Fr;
use ark_crypto_primitives::sponge::CryptographicSponge;
use ark_crypto_primitives::sponge::constraints::CryptographicSpongeVar;
use ark_crypto_primitives::sponge::poseidon::PoseidonSponge;
use ark_crypto_primitives::sponge::poseidon::constraints::PoseidonSpongeVar;
use ark_r1cs_std::fields::fp::FpVar;
use ark_relations::gr1cs::{ConstraintSystemRef, SynthesisError};

use crate::params::poseidon_config;

/// Hashes a sequence of field elements to one field element.
#[must_use]
pub fn hash(inputs: &[Fr]) -> Fr {
    let mut sponge = PoseidonSponge::new(poseidon_config());
    sponge.absorb(&inputs);
    sponge.squeeze_field_elements::<Fr>(1)[0]
}

/// The in-circuit counterpart of [`hash`].
///
/// # Errors
///
/// Propagates constraint synthesis failures.
pub fn hash_var(
    cs: ConstraintSystemRef<Fr>,
    inputs: &[FpVar<Fr>],
) -> Result<FpVar<Fr>, SynthesisError> {
    let mut sponge = PoseidonSpongeVar::new(cs, poseidon_config());
    sponge.absorb(&inputs)?;
    Ok(sponge.squeeze_field_elements(1)?[0].clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_r1cs_std::GR1CSVar;
    use ark_r1cs_std::alloc::AllocVar;
    use ark_relations::gr1cs::ConstraintSystem;

    #[test]
    fn native_and_circuit_agree() {
        // Varying arity matters: the sponge pads differently either side of a
        // rate boundary, and a mismatch there would only show up for some note
        // shapes.
        for arity in 1..=6usize {
            let inputs: Vec<Fr> = (0..arity).map(|i| Fr::from(i as u64 + 1)).collect();
            let expected = hash(&inputs);

            let cs = ConstraintSystem::<Fr>::new_ref();
            let vars: Vec<FpVar<Fr>> = inputs
                .iter()
                .map(|value| FpVar::new_witness(cs.clone(), || Ok(*value)).expect("witness"))
                .collect();
            let actual = hash_var(cs.clone(), &vars).expect("hash in circuit");

            assert!(cs.is_satisfied().expect("satisfiable"));
            assert_eq!(
                actual.value().expect("value"),
                expected,
                "native and circuit disagree at arity {arity}"
            );
        }
    }

    #[test]
    fn different_inputs_hash_differently() {
        assert_ne!(hash(&[Fr::from(1u64)]), hash(&[Fr::from(2u64)]));
    }

    #[test]
    fn ordering_is_significant() {
        let a = [Fr::from(1u64), Fr::from(2u64)];
        let b = [Fr::from(2u64), Fr::from(1u64)];
        assert_ne!(hash(&a), hash(&b));
    }
}
