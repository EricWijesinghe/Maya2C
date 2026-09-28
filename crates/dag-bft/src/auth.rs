//! Who signed what: the one place the engine touches cryptography.
//!
//! The engine stays sans-IO *and* sans-crypto-library: it asks an
//! [`Authenticator`] to sign a vertex digest and to check someone else's
//! signature over one. The node implements it with ML-DSA-65 keys taken from
//! genesis and the staking set; the simulator uses [`Unauthenticated`], which
//! signs nothing and believes everything, and says so.
//!
//! A proposal's signature and its author's vote are the same object — a
//! signature over the vertex digest — so a certificate proves both that 2f + 1
//! validators saw the vertex and that its author made it.

use crate::vertex::{Digest, ValidatorId, Vertex};

/// What a signature is for (ADR-032). Never signed itself: the signed bytes
/// are the digest alone, so this changes no consensus rule. It exists so an
/// authenticator that enforces slashing protection, such as the remote
/// signer, can know which `(round, author)` slot it is being asked to fill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignKind {
    /// This validator's own vertex.
    Proposal,
    /// A vote for `author`'s vertex.
    Vote,
}

/// The slot a signature fills: the one fact slashing protection needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignContext {
    /// Proposal or vote.
    pub kind: SignKind,
    /// The vertex's round.
    pub round: u64,
    /// The vertex's author (this validator itself for a proposal).
    pub author: ValidatorId,
}

/// Signing with this validator's key and verifying the committee's.
pub trait Authenticator {
    /// This validator's signature over `digest`, which fills the slot `ctx`
    /// names. An empty result means "not signed" and costs this validator
    /// its vote, never safety.
    fn sign(&self, ctx: SignContext, digest: &Digest) -> Vec<u8>;

    /// Whether `signature` is `signer`'s over `digest`. Must be false for a
    /// signer outside the committee.
    fn verify(&self, signer: ValidatorId, digest: &Digest, signature: &[u8]) -> bool;
}

/// SIM: no keys. Every signature is empty and every signature verifies.
///
/// Correct for the deterministic simulator, whose adversary is the network
/// (loss, delay, partition, crash), never a forger. Never on a production
/// path: the node constructs its engine only through an authenticator backed
/// by real keys.
#[derive(Clone, Copy, Debug, Default)]
pub struct Unauthenticated;

impl Authenticator for Unauthenticated {
    fn sign(&self, _ctx: SignContext, _digest: &Digest) -> Vec<u8> {
        Vec::new()
    }

    fn verify(&self, _signer: ValidatorId, _digest: &Digest, _signature: &[u8]) -> bool {
        true
    }
}

/// Two validly signed vertices by one author for one round: the evidence the
/// staking module slashes on. Certification makes equivocation harmless to
/// safety (only one of the two can gather a quorum) but not free, and this is
/// what makes it cost the author its bond.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Equivocation {
    /// The vertex seen first.
    pub first: Vertex,
    /// Its author's signature.
    pub first_signature: Vec<u8>,
    /// The conflicting vertex.
    pub second: Vertex,
    /// Its author's signature.
    pub second_signature: Vec<u8>,
}

impl Equivocation {
    /// Whether this is real evidence under `auth`: same author, epoch and
    /// round, different digests, both signatures valid. Checked again by the
    /// state machine before anything is slashed — a validator's own engine
    /// reporting it is not proof.
    pub fn is_valid<A: Authenticator>(&self, auth: &A) -> bool {
        let (a, b) = (&self.first, &self.second);
        let (da, db) = (a.digest(), b.digest());
        a.author == b.author
            && a.epoch == b.epoch
            && a.round == b.round
            && da != db
            && auth.verify(a.author, &da, &self.first_signature)
            && auth.verify(b.author, &db, &self.second_signature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Signs with the id; verifies the id matches. Enough to exercise the
    /// evidence rule without a signature scheme.
    struct ById(ValidatorId);

    impl Authenticator for ById {
        fn sign(&self, _ctx: SignContext, _digest: &Digest) -> Vec<u8> {
            self.0.to_le_bytes().to_vec()
        }

        fn verify(&self, signer: ValidatorId, _digest: &Digest, signature: &[u8]) -> bool {
            signature == signer.to_le_bytes()
        }
    }

    fn vertex(author: ValidatorId, round: u64, stamp: u64) -> Vertex {
        let mut v = Vertex::genesis_in(0, author);
        v.round = round;
        v.timestamp_ms = stamp;
        v
    }

    #[test]
    fn equivocation_needs_one_author_one_round_two_digests_two_signatures() {
        let auth = ById(0);
        let sig = |a: ValidatorId| a.to_le_bytes().to_vec();
        let ev = |a: Vertex, b: Vertex, sa, sb| Equivocation {
            first: a,
            first_signature: sa,
            second: b,
            second_signature: sb,
        };
        assert!(ev(vertex(2, 5, 1), vertex(2, 5, 2), sig(2), sig(2)).is_valid(&auth));
        // Same vertex twice is not a conflict.
        assert!(!ev(vertex(2, 5, 1), vertex(2, 5, 1), sig(2), sig(2)).is_valid(&auth));
        // Different rounds or authors are not a conflict.
        assert!(!ev(vertex(2, 5, 1), vertex(2, 6, 2), sig(2), sig(2)).is_valid(&auth));
        assert!(!ev(vertex(2, 5, 1), vertex(3, 5, 2), sig(2), sig(3)).is_valid(&auth));
        // A forged signature is not evidence.
        assert!(!ev(vertex(2, 5, 1), vertex(2, 5, 2), sig(2), sig(1)).is_valid(&auth));
    }
}
