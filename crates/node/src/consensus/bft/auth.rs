//! ML-DSA-65 behind the engine's [`Authenticator`].

use std::sync::Arc;

use maya_dag_bft::{Authenticator, Digest, SignContext, ValidatorId};

use crate::crypto::SIGNATURE_LENGTH;
use crate::crypto::keys::{SigningKey, VerifyingKey};

/// Prefixed to every digest a validator signs. The vertex digest already has
/// its own BLAKE3 domain; this one makes the *signature* unusable anywhere
/// else a validator key might sign — a transaction, a peer handshake — even if
/// an operator reused the key, which they should not.
const VOTE_DOMAIN: &[u8] = b"maya2c/dag-bft/vote/v1";

/// A validator's signing key and its committee's verifying keys.
///
/// Observers hold no signing key: they verify, and an engine running as an
/// observer never asks them to sign.
#[derive(Clone)]
pub struct MlDsaAuthenticator {
    signer: Option<Arc<SigningKey>>,
    committee: Arc<[VerifyingKey]>,
}

impl MlDsaAuthenticator {
    /// A voting validator's authenticator.
    #[must_use]
    pub fn validator(signer: Arc<SigningKey>, committee: Arc<[VerifyingKey]>) -> Self {
        Self {
            signer: Some(signer),
            committee,
        }
    }

    /// An observer's: verifies, never signs.
    #[must_use]
    pub fn observer(committee: Arc<[VerifyingKey]>) -> Self {
        Self {
            signer: None,
            committee,
        }
    }

    /// The committee, in validator-id order.
    #[must_use]
    pub fn committee(&self) -> &[VerifyingKey] {
        &self.committee
    }

    fn message(digest: &Digest) -> Vec<u8> {
        let mut m = Vec::with_capacity(VOTE_DOMAIN.len() + digest.len());
        m.extend_from_slice(VOTE_DOMAIN);
        m.extend_from_slice(digest);
        m
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
    fn sign(&self, _ctx: SignContext, digest: &Digest) -> Vec<u8> {
        // A local key has its own protection: the node's safety log. An empty signature verifies nowhere, so a failure here costs this
        // validator its vote and nothing else. FIPS 204 lets the rejection
        // loop report failure; it is not a condition to retry.
        self.signer
            .as_ref()
            .and_then(|key| key.sign(&Self::message(digest)).ok())
            .map(|sig| sig.to_vec())
            .unwrap_or_default()
    }

    fn verify(&self, signer: ValidatorId, digest: &Digest, signature: &[u8]) -> bool {
        let Some(key) = self.committee.get(usize::from(signer)) else {
            return false;
        };
        let Ok(sig) = <&[u8; SIGNATURE_LENGTH]>::try_from(signature) else {
            return false;
        };
        key.verify(&Self::message(digest), sig).is_ok()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::crypto::keys::signing_key_from_seed;

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
        let auth = MlDsaAuthenticator::validator(Arc::clone(&signers[1]), Arc::clone(&committee));
        let digest = [7u8; 32];
        let sig = auth.sign(CTX, &digest);
        assert_eq!(sig.len(), SIGNATURE_LENGTH);
        let observer = MlDsaAuthenticator::observer(committee);
        assert!(observer.verify(1, &digest, &sig));
        assert!(!observer.verify(0, &digest, &sig), "someone else's key");
        assert!(!observer.verify(1, &[8u8; 32], &sig), "another digest");
        assert!(!observer.verify(9, &digest, &sig), "outside the committee");
        assert!(!observer.verify(1, &digest, &sig[1..]), "truncated");
    }

    #[test]
    fn an_observer_signs_nothing() {
        let (_, committee) = keys(1);
        assert!(
            MlDsaAuthenticator::observer(committee)
                .sign(CTX, &[0; 32])
                .is_empty()
        );
    }
}
