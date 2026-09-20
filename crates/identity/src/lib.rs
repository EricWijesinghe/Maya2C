//! Self-sovereign identity for Maya2C: who a subject is, which key speaks for
//! them, and what an issuer has attested — without the chain ever learning a
//! fact about a person.
//!
//! ## The one rule
//!
//! **No claim preimage reaches chain state, ever.** Only commitments and roots.
//!
//! A chain is permanent and public. A date of birth written to it once is
//! written forever, and no later fix reaches it — not a migration, not a
//! governance vote, not a hard fork, because every archive node already has it.
//! So the types here are shaped so that putting a claim on chain is not a
//! mistake somebody could make. There is no field for one, in any record. See
//! [`attestation`].
//!
//! ## `did:maya2c:<address>`
//!
//! The brief said `did:maya2c:<pubkey>`. A Maya2C public key is a hybrid pair
//! at 1,984 bytes — about 2,712 base58 characters, which is not an identifier.
//! The chain's address is BLAKE3 over *both* keys and is 44 characters, and it
//! inherits the binding argument `Transaction::sender` already makes. Keys live
//! in the document. See [`did`].
//!
//! ## Rotation and revocation need no new cryptography
//!
//! ML-DSA-65 is already a lattice signature and every transaction carries one.
//! A rotation authorised by the *old* key signing the new one is the right
//! construction and is an ordinary state transition. A second proof system
//! would be another thing to get wrong for no gain.
//!
//! ## What is post-quantum here, and what is not
//!
//! | | |
//! |---|---|
//! | Subject and issuer keys | ML-DSA-65 + SLH-DSA — post-quantum |
//! | An issuer's attestation | signed with that pair, verified natively by consensus — post-quantum |
//! | A holder's disclosure proof | Groth16 over BLS12-381 — **not** post-quantum |
//!
//! The split is not an oversight. Verifying ML-DSA inside a circuit would be
//! 10^7 to 10^8 constraints, so the issuer's signature is checked by the chain
//! and never enters one; what the circuit proves is membership under a root the
//! chain already vouches for. That leaves the *presentation* forgeable by a
//! quantum adversary even though the attestation is not — the same standing
//! caveat as the shielded pool, stated here rather than left to be inferred.
//!
//! ## Chain-free
//!
//! Nothing here knows what a block is. A DID Document arrives from a stranger —
//! resolved from another node, handed over by a wallet, read out of a QR code —
//! so the decoder is fuzzable without RocksDB in the graph, the same boundary
//! `iso20022` and `radio-transport` hold.

pub mod attestation;
pub mod did;
pub mod document;
pub mod error;

pub use attestation::{Commitment, CryptographicAttestation, RevocationPage, Root};
pub use did::Did;
pub use document::{DidDocument, ServiceEndpoint, VerificationMethod};
pub use error::{Error, Result};

/// Domain separating a credential commitment from every other digest.
const DOMAIN_CREDENTIAL: &[u8] = b"maya-identity-credential-v1:";

/// The commitment an issuer puts in its tree for one claim.
///
/// `H(domain || subject || schema || value || blinding)`.
///
/// The blinding factor is what makes this a commitment rather than a lookup
/// table. Without it, a verifier holding the tree could test "is this subject's
/// age 34?" by recomputing the digest — and with a few dozen guesses would have
/// the value. Ages, countries and accreditation statuses all come from small
/// sets, which is exactly the case an unblinded hash fails.
///
/// The value is a `u64` because every predicate this supports is an inequality
/// or an equality over one: an age, a country code, a boolean. A string here
/// would need a canonicalisation rule, and two implementations disagreeing
/// about one is a credential that verifies on one verifier and not the next.
#[must_use]
pub fn commitment(
    subject: &Did,
    schema: &str,
    value: u64,
    blinding: &[u8; 32],
) -> attestation::Commitment {
    let mut hasher = blake3::Hasher::new();
    hasher.update(DOMAIN_CREDENTIAL);
    hasher.update(&subject.address());
    hasher.update(&(schema.len() as u16).to_le_bytes());
    hasher.update(schema.as_bytes());
    hasher.update(&value.to_le_bytes());
    hasher.update(blinding);
    *hasher.finalize().as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subject() -> Did {
        Did::from_address([5; 32])
    }

    #[test]
    fn one_claim_has_one_commitment() {
        let blinding = [9u8; 32];
        assert_eq!(
            commitment(&subject(), "age", 34, &blinding),
            commitment(&subject(), "age", 34, &blinding)
        );
    }

    #[test]
    fn the_blinding_factor_is_what_stops_a_guessing_attack() {
        // Ages, country codes and accreditation flags all come from small sets.
        // Without a blinding factor a verifier holding the tree recovers the
        // value by recomputing a few dozen digests.
        let first = commitment(&subject(), "age", 34, &[1u8; 32]);
        let second = commitment(&subject(), "age", 34, &[2u8; 32]);
        assert_ne!(first, second);
    }

    #[test]
    fn every_field_changes_the_commitment() {
        let base = commitment(&subject(), "age", 34, &[0u8; 32]);
        assert_ne!(
            base,
            commitment(&Did::from_address([6; 32]), "age", 34, &[0u8; 32])
        );
        assert_ne!(base, commitment(&subject(), "height", 34, &[0u8; 32]));
        assert_ne!(base, commitment(&subject(), "age", 35, &[0u8; 32]));
    }

    #[test]
    fn a_schema_boundary_cannot_be_shifted() {
        // Without the length prefix, ("ab", 1) and ("a", …) could collide by
        // moving a byte across the boundary — which is one subject's claim
        // reading as another's.
        assert_ne!(
            commitment(&subject(), "ab", 0, &[0u8; 32]),
            commitment(&subject(), "a", 0, &[0u8; 32])
        );
    }
}
