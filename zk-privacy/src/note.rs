//! Shielded notes: the coins in the pool.
//!
//! A note is a private record of value. What reaches the chain is only its
//! *commitment* — a Poseidon hash binding the value, the owner, and two random
//! blinding factors. The commitment reveals nothing, and it is hiding because
//! `rand` is uniform: two notes of the same value to the same owner have
//! unrelated commitments.
//!
//! ## Spending
//!
//! Spending publishes a *nullifier* derived from the spending key and the
//! note's `rho`. It is unlinkable to the commitment without the spending key,
//! so an observer cannot tell which note was consumed — but it is deterministic
//! for a given note, so spending the same note twice publishes the same
//! nullifier and the second attempt is rejected.
//!
//! `rho` must be unique per note. Two notes sharing a `rho` under one spending
//! key produce the same nullifier, which makes the second unspendable.

use ark_bls12_381::Fr;

use crate::hash::hash;
use crate::params::{DOMAIN_ADDRESS, DOMAIN_COMMITMENT, DOMAIN_NULLIFIER};

/// A spending key. Whoever holds this can spend notes sent to its address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpendingKey(pub Fr);

impl SpendingKey {
    /// The shielded address that receives notes for this key.
    #[must_use]
    pub fn address(&self) -> Address {
        Address(hash(&[Fr::from(DOMAIN_ADDRESS), self.0]))
    }
}

/// A shielded address: the public half of a [`SpendingKey`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Address(pub Fr);

/// A note: value held by an address, blinded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Note {
    /// Value in base units. Constrained to 64 bits by the circuit.
    pub value: u64,
    /// Owning shielded address.
    pub address: Address,
    /// Nullifier randomness. Must be unique per note.
    pub rho: Fr,
    /// Commitment blinding factor. Makes the commitment hiding.
    pub rand: Fr,
}

impl Note {
    /// Builds a note.
    #[must_use]
    pub fn new(value: u64, address: Address, rho: Fr, rand: Fr) -> Self {
        Self {
            value,
            address,
            rho,
            rand,
        }
    }

    /// A zero-value note, used to pad a joinsplit that has fewer real inputs
    /// or outputs than the fixed arity.
    ///
    /// Dummy notes are indistinguishable on chain from real ones: they publish
    /// a commitment and a nullifier like any other. That is the point — a
    /// one-input spend must not be recognisable as such.
    #[must_use]
    pub fn dummy(rho: Fr, rand: Fr) -> Self {
        Self {
            value: 0,
            address: Address(Fr::from(0u64)),
            rho,
            rand,
        }
    }

    /// The commitment published on chain.
    #[must_use]
    pub fn commitment(&self) -> Fr {
        hash(&[
            Fr::from(DOMAIN_COMMITMENT),
            Fr::from(self.value),
            self.address.0,
            self.rho,
            self.rand,
        ])
    }

    /// The nullifier revealed when this note is spent.
    ///
    /// Derived from the spending key rather than the note alone, so only the
    /// owner can compute it in advance. An observer holding the commitment
    /// cannot link it to the nullifier that eventually retires it.
    #[must_use]
    pub fn nullifier(&self, key: &SpendingKey) -> Fr {
        hash(&[Fr::from(DOMAIN_NULLIFIER), key.0, self.rho])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u64) -> SpendingKey {
        SpendingKey(Fr::from(seed))
    }

    #[test]
    fn an_address_is_determined_by_its_spending_key() {
        assert_eq!(key(1).address(), key(1).address());
        assert_ne!(key(1).address(), key(2).address());
    }

    #[test]
    fn blinding_hides_equal_notes() {
        let address = key(1).address();
        let a = Note::new(100, address, Fr::from(7u64), Fr::from(11u64));
        let b = Note::new(100, address, Fr::from(7u64), Fr::from(12u64));
        // Same value, same owner, same rho — only `rand` differs.
        assert_ne!(a.commitment(), b.commitment());
    }

    #[test]
    fn commitment_binds_every_field() {
        let address = key(1).address();
        let base = Note::new(100, address, Fr::from(7u64), Fr::from(11u64));

        let other_value = Note { value: 101, ..base };
        let other_owner = Note {
            address: key(2).address(),
            ..base
        };
        let other_rho = Note {
            rho: Fr::from(8u64),
            ..base
        };

        assert_ne!(base.commitment(), other_value.commitment());
        assert_ne!(base.commitment(), other_owner.commitment());
        assert_ne!(base.commitment(), other_rho.commitment());
    }

    #[test]
    fn a_nullifier_is_stable_for_one_note_and_key() {
        let note = Note::new(5, key(1).address(), Fr::from(3u64), Fr::from(4u64));
        assert_eq!(note.nullifier(&key(1)), note.nullifier(&key(1)));
    }

    #[test]
    fn distinct_rho_gives_distinct_nullifiers() {
        let address = key(1).address();
        let a = Note::new(5, address, Fr::from(3u64), Fr::from(4u64));
        let b = Note::new(5, address, Fr::from(9u64), Fr::from(4u64));
        assert_ne!(a.nullifier(&key(1)), b.nullifier(&key(1)));
    }

    #[test]
    fn a_nullifier_does_not_reveal_the_commitment() {
        // Not a security proof — just a guard that the two are not accidentally
        // the same value, which would link spends to notes outright.
        let note = Note::new(5, key(1).address(), Fr::from(3u64), Fr::from(4u64));
        assert_ne!(note.nullifier(&key(1)), note.commitment());
    }

    #[test]
    fn a_dummy_note_carries_no_value() {
        assert_eq!(Note::dummy(Fr::from(1u64), Fr::from(2u64)).value, 0);
    }
}
