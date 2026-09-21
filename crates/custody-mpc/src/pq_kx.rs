//! `X25519MLKEM768` for the custody TLS hop: the hybrid key exchange of
//! draft-ietf-tls-ecdhe-mlkem, codepoint `0x11EC`.
//!
//! # Why the hop needs it when the shares are already sealed
//!
//! Shares cross this link sealed to their recipient under ML-KEM-768
//! ([`crate::seal`]), so their confidentiality never depended on TLS. What
//! did is everything around them — which custodians took part in which
//! session, and when — and with the `ring` provider's classical groups a
//! recording of today's traffic is readable by whoever holds a quantum
//! computer later. Offering only this group closes that.
//!
//! # What it does not change
//!
//! Peer *authentication* is still the certificates' signatures, which are
//! classical: `webpki` verifies no ML-DSA certificate. Breaking that needs a
//! quantum adversary active during the handshake, not one replaying a
//! recording, and every custodian contribution is additionally bound to its
//! session inside the sealed payload. Stated so nobody reads "PQ TLS" as more
//! than it is.
//!
//! # Encoding
//!
//! Client share `ek ‖ x25519_pub` (1184 + 32), server share
//! `ciphertext ‖ x25519_pub` (1088 + 32), secret `ml_kem_ss ‖ x25519_ss` —
//! ML-KEM first, as the draft orders it for this group.

use curve25519_dalek::montgomery::MontgomeryPoint;
use maya_crypto_pq::kem::{
    self, CIPHERTEXT_LEN, DecapsulationKey, ENCAPSULATION_KEY_LEN, EncapsulationKey,
};
use rustls::crypto::{ActiveKeyExchange, CompletedKeyExchange, SharedSecret, SupportedKxGroup};
use rustls::{Error, NamedGroup, PeerMisbehaved, ProtocolVersion};
use zeroize::Zeroizing;

/// X25519 scalar and point width.
const X25519_LEN: usize = 32;

/// The group, for a provider's `kx_groups`.
pub static X25519_MLKEM768: &dyn SupportedKxGroup = &X25519MlKem768;

/// The `X25519MLKEM768` group.
#[derive(Debug)]
pub struct X25519MlKem768;

/// A fresh X25519 keypair from the OS CSPRNG.
fn x25519_keypair() -> Result<(Zeroizing<[u8; X25519_LEN]>, [u8; X25519_LEN]), Error> {
    let mut secret = Zeroizing::new([0u8; X25519_LEN]);
    getrandom::fill(secret.as_mut_slice())
        .map_err(|_| Error::General("no entropy for an X25519 key".into()))?;
    let public = MontgomeryPoint::mul_base_clamped(*secret).to_bytes();
    Ok((secret, public))
}

/// The X25519 shared secret, refusing the all-zero output a low-order peer
/// point produces (RFC 7748 §6.1).
fn x25519(secret: &[u8; X25519_LEN], peer: &[u8]) -> Result<Zeroizing<[u8; X25519_LEN]>, Error> {
    let peer: [u8; X25519_LEN] = peer
        .try_into()
        .map_err(|_| PeerMisbehaved::InvalidKeyShare)?;
    let shared = Zeroizing::new(MontgomeryPoint(peer).mul_clamped(*secret).to_bytes());
    if shared.iter().all(|b| *b == 0) {
        return Err(PeerMisbehaved::InvalidKeyShare.into());
    }
    Ok(shared)
}

/// `first ‖ second`, as the secret rustls feeds its key schedule.
fn concat(first: &[u8], second: &[u8]) -> SharedSecret {
    let mut secret = Vec::with_capacity(first.len() + second.len());
    secret.extend_from_slice(first);
    secret.extend_from_slice(second);
    SharedSecret::from(secret)
}

impl SupportedKxGroup for X25519MlKem768 {
    fn start(&self) -> Result<Box<dyn ActiveKeyExchange>, Error> {
        let (decaps, encaps) = kem::generate_keypair();
        let (x_secret, x_public) = x25519_keypair()?;
        let mut share = Vec::with_capacity(ENCAPSULATION_KEY_LEN + X25519_LEN);
        share.extend_from_slice(&encaps.to_bytes());
        share.extend_from_slice(&x_public);
        Ok(Box::new(ClientExchange {
            decaps,
            x_secret,
            share,
        }))
    }

    /// The server side: encapsulate to the client's key, then DH.
    fn start_and_complete(&self, client_share: &[u8]) -> Result<CompletedKeyExchange, Error> {
        if client_share.len() != ENCAPSULATION_KEY_LEN + X25519_LEN {
            return Err(PeerMisbehaved::InvalidKeyShare.into());
        }
        let (ek_bytes, x_peer) = client_share.split_at(ENCAPSULATION_KEY_LEN);
        let ek_bytes: &[u8; ENCAPSULATION_KEY_LEN] = ek_bytes
            .try_into()
            .map_err(|_| PeerMisbehaved::InvalidKeyShare)?;
        let encaps =
            EncapsulationKey::from_bytes(ek_bytes).map_err(|_| PeerMisbehaved::InvalidKeyShare)?;
        let (ciphertext, kem_secret) = encaps.encapsulate();
        let (x_secret, x_public) = x25519_keypair()?;
        let x_shared = x25519(&x_secret, x_peer)?;

        let mut pub_key = Vec::with_capacity(CIPHERTEXT_LEN + X25519_LEN);
        pub_key.extend_from_slice(&ciphertext);
        pub_key.extend_from_slice(&x_public);
        Ok(CompletedKeyExchange {
            group: self.name(),
            pub_key,
            secret: concat(kem_secret.as_bytes(), x_shared.as_slice()),
        })
    }

    fn name(&self) -> NamedGroup {
        NamedGroup::X25519MLKEM768
    }

    fn usable_for_version(&self, version: ProtocolVersion) -> bool {
        version == ProtocolVersion::TLSv1_3
    }
}

/// The client's half, between sending its share and reading the server's.
struct ClientExchange {
    decaps: DecapsulationKey,
    x_secret: Zeroizing<[u8; X25519_LEN]>,
    share: Vec<u8>,
}

impl ActiveKeyExchange for ClientExchange {
    fn complete(self: Box<Self>, server_share: &[u8]) -> Result<SharedSecret, Error> {
        if server_share.len() != CIPHERTEXT_LEN + X25519_LEN {
            return Err(PeerMisbehaved::InvalidKeyShare.into());
        }
        let (ct, x_peer) = server_share.split_at(CIPHERTEXT_LEN);
        let ct: &[u8; CIPHERTEXT_LEN] =
            ct.try_into().map_err(|_| PeerMisbehaved::InvalidKeyShare)?;
        // Decapsulation never fails (FIPS 203 implicit rejection); a forged
        // ciphertext yields a secret the server does not share, and the
        // handshake dies at Finished.
        let kem_secret = self.decaps.decapsulate(ct);
        let x_shared = x25519(&self.x_secret, x_peer)?;
        Ok(concat(kem_secret.as_bytes(), x_shared.as_slice()))
    }

    fn pub_key(&self) -> &[u8] {
        &self.share
    }

    fn group(&self) -> NamedGroup {
        NamedGroup::X25519MLKEM768
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn both_sides_derive_the_same_64_byte_secret() {
        let client = X25519_MLKEM768.start().expect("start");
        let server = X25519_MLKEM768
            .start_and_complete(client.pub_key())
            .expect("server");
        let client_secret = client.complete(&server.pub_key).expect("client");
        assert_eq!(client_secret.secret_bytes().len(), 64);
        assert_eq!(client_secret.secret_bytes(), server.secret.secret_bytes());
    }

    #[test]
    fn a_low_order_x25519_point_is_refused() {
        let client = X25519_MLKEM768.start().expect("start");
        let mut share = client.pub_key().to_vec();
        let len = share.len();
        share[len - X25519_LEN..].fill(0); // the identity: every output is zero
        assert!(X25519_MLKEM768.start_and_complete(&share).is_err());
    }

    #[test]
    fn a_truncated_share_is_refused() {
        let client = X25519_MLKEM768.start().expect("start");
        let share = &client.pub_key()[..ENCAPSULATION_KEY_LEN];
        assert!(X25519_MLKEM768.start_and_complete(share).is_err());
    }

    #[test]
    fn a_tampered_ciphertext_breaks_agreement_rather_than_erroring() {
        let client = X25519_MLKEM768.start().expect("start");
        let server = X25519_MLKEM768
            .start_and_complete(client.pub_key())
            .expect("server");
        let mut reply = server.pub_key.clone();
        reply[0] ^= 1;
        let client_secret = client.complete(&reply).expect("implicit rejection");
        assert_ne!(client_secret.secret_bytes(), server.secret.secret_bytes());
    }
}
