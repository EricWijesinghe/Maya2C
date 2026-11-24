//! ML-DSA-87 validator keys (CRY-4) behind the engine's [`Authenticator`].

use std::sync::Arc;

use maya_dag_bft::{Authenticator, Digest, SignContext, ValidatorId};

use super::remote::ValidatorKey;
use crate::crypto::validator_keys::SIGNATURE_LENGTH;
use crate::crypto::validator_keys::VerifyingKey;

use maya_dag_bft::{VOTE_DOMAIN, slot_message};

use crate::core::ChainTag;

/// A validator's signing key and its committee's verifying keys.
///
/// Observers hold no signing key: they verify, and an engine running as an
/// observer never asks them to sign.
#[derive(Clone)]
pub struct MlDsaAuthenticator {
    signer: Option<ValidatorKey>,
    committee: Arc<[VerifyingKey]>,
    /// The chain every vote is bound to (pre-audit HIGH-4): a key reused on a
    /// testnet cannot have its testnet votes count, or slash, on mainnet.
    chain: ChainTag,
}

impl MlDsaAuthenticator {
    /// A voting validator's authenticator, signing with a local key or
    /// through the remote signer (ADR-033).
    #[must_use]
    pub fn validator(
        signer: impl Into<ValidatorKey>,
        committee: Arc<[VerifyingKey]>,
        chain: ChainTag,
    ) -> Self {
        Self {
            signer: Some(signer.into()),
            committee,
            chain,
        }
    }

    /// An observer's: verifies, never signs.
    #[must_use]
    pub fn observer(committee: Arc<[VerifyingKey]>, chain: ChainTag) -> Self {
        Self {
            signer: None,
            committee,
            chain,
        }
    }

    /// The committee, in validator-id order.
    #[must_use]
    pub fn committee(&self) -> &[VerifyingKey] {
        &self.committee
    }

    /// The engine's digest bound to this chain. What is signed is
    /// `VOTE_DOMAIN ‖ round ‖ author ‖ bound` ([`slot_message`], CT-05), and a
    /// remote signer is handed `bound` as the digest with the slot beside it,
    /// so local keys and remote signers still sign the same bytes.
    fn bound(&self, digest: &Digest) -> Digest {
        let mut h = blake3::Hasher::new_derive_key("maya2c dag-bft vote chain v1");
        h.update(&self.chain.0);
        h.update(digest);
        *h.finalize().as_bytes()
    }

    fn message(round: u64, author: ValidatorId, bound: &Digest) -> Vec<u8> {
        slot_message(VOTE_DOMAIN, round, author, bound)
    }
}

impl core::fmt::Debug for MlDsaAuthenticator {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("MlDsaAuthenticator")
            .field("signs", &self.signer.is_some())
            .field("committee", &self.committee.len())
            .finish()
    }
}

impl Authenticator for MlDsaAuthenticator {
    fn sign(&self, ctx: SignContext, digest: &Digest) -> Vec<u8> {
        // An empty signature verifies nowhere, so a failure here costs this
        // validator its vote and nothing else. FIPS 204 lets the rejection
        // loop report failure; it is not a condition to retry. A local key's
        // protection is the node's safety log; a remote signer adds its own
        // and needs `ctx` for it.
        let bound = self.bound(digest);
        match &self.signer {
            None => Vec::new(),
            Some(ValidatorKey::Local(key)) => key
                .sign(&Self::message(ctx.round, ctx.author, &bound))
                .map(|sig| sig.to_vec())
                .unwrap_or_default(),
            Some(ValidatorKey::Remote(remote)) => remote.sign(ctx, &bound).unwrap_or_default(),
        }
    }

    fn verify(
        &self,
        signer: ValidatorId,
        round: u64,
        author: ValidatorId,
        digest: &Digest,
        signature: &[u8],
    ) -> bool {
        let Some(key) = self.committee.get(usize::from(signer)) else {
            return false;
        };
        let Ok(sig) = <&[u8; SIGNATURE_LENGTH]>::try_from(signature) else {
            return false;
        };
        key.verify(&Self::message(round, author, &self.bound(digest)), sig)
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    const TAG: ChainTag = ChainTag([1; 32]);
    use crate::crypto::validator_keys::{SigningKey, signing_key_from_seed};

    const CTX: SignContext = SignContext {
        kind: maya_dag_bft::SignKind::Vote,
        round: 1,
        author: 0,
    };

    fn keys(n: u8) -> (Vec<Arc<SigningKey>>, Arc<[VerifyingKey]>) {
        let signers: Vec<_> = (0..n)
            .map(|i| Arc::new(signing_key_from_seed(&[i; 32]).unwrap()))
            .collect();
        let committee: Arc<[VerifyingKey]> = signers.iter().map(|k| k.verifying_key()).collect();
        (signers, committee)
    }

    #[test]
    fn a_vote_verifies_only_for_its_signer_and_its_digest() {
        let (signers, committee) = keys(3);
        let auth =
            MlDsaAuthenticator::validator(Arc::clone(&signers[1]), Arc::clone(&committee), TAG);
        let digest = [7u8; 32];
        let sig = auth.sign(CTX, &digest);
        assert_eq!(sig.len(), SIGNATURE_LENGTH);
        let observer = MlDsaAuthenticator::observer(committee, TAG);
        let (r, a) = (CTX.round, CTX.author);
        assert!(observer.verify(1, r, a, &digest, &sig));
        assert!(
            !observer.verify(0, r, a, &digest, &sig),
            "someone else's key"
        );
        assert!(
            !observer.verify(1, r, a, &[8u8; 32], &sig),
            "another digest"
        );
        assert!(
            !observer.verify(9, r, a, &digest, &sig),
            "outside the committee"
        );
        assert!(!observer.verify(1, r, a, &digest, &sig[1..]), "truncated");
    }

    #[test]
    fn a_signature_fills_only_the_slot_it_was_made_for() {
        // CT-05: the slot is signed, so a signature made under one round or
        // author label is no signature for the same digest at another slot.
        let (signers, committee) = keys(2);
        let auth =
            MlDsaAuthenticator::validator(Arc::clone(&signers[0]), Arc::clone(&committee), TAG);
        let digest = [5u8; 32];
        let sig = auth.sign(CTX, &digest);
        let observer = MlDsaAuthenticator::observer(committee, TAG);
        assert!(observer.verify(0, CTX.round, CTX.author, &digest, &sig));
        assert!(
            !observer.verify(0, CTX.round + 1, CTX.author, &digest, &sig),
            "another round"
        );
        assert!(
            !observer.verify(0, CTX.round, CTX.author + 1, &digest, &sig),
            "another author"
        );
    }

    #[test]
    fn an_observer_signs_nothing() {
        let (_, committee) = keys(1);
        assert!(
            MlDsaAuthenticator::observer(committee, TAG)
                .sign(CTX, &[0; 32])
                .is_empty()
        );
    }

    #[test]
    fn a_vote_signed_for_one_chain_does_not_verify_on_another() {
        // Pre-audit HIGH-4: a validator key reused on a testnet must not let
        // its testnet votes (or equivocations) count on mainnet.
        let key = Arc::new(crate::crypto::validator_keys::signing_key_from_seed(&[3; 32]).unwrap());
        let committee: Arc<[VerifyingKey]> = vec![key.verifying_key()].into();
        let testnet = MlDsaAuthenticator::validator(Arc::clone(&key), Arc::clone(&committee), TAG);
        let mainnet = MlDsaAuthenticator::observer(committee, ChainTag([2; 32]));
        let digest = [9u8; 32];
        let sig = testnet.sign(CTX, &digest);
        assert!(testnet.verify(0, CTX.round, CTX.author, &digest, &sig));
        assert!(!mainnet.verify(0, CTX.round, CTX.author, &digest, &sig));
    }
}
