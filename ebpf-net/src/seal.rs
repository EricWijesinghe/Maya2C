//! Relay keys, and sealing one chunk under one.
//!
//! # Where a relay key comes from
//!
//! Each peer contributes 32 random bytes over the libp2p connection they
//! already share (`/maya/relay-key/1.0.0`, `src/network/relay_key.rs`). That
//! connection is Noise *and* the ML-KEM-768 layer, so the contributions — and
//! therefore the key — are exactly as confidential as gossip is: an attacker
//! recording today has to break both X25519 and ML-KEM to recover them. The
//! peer on the other end is the one Noise authenticated, so a chunk that opens
//! under the key came from that `PeerId`.
//!
//! [`derive_keys`] turns the two contributions and both peer ids into two keys,
//! one per direction. Two, never one: a shared key would put both peers' nonces
//! in one space.
//!
//! # Nonces
//!
//! XChaCha20-Poly1305, with the 24-byte nonce derived from the whole 56-byte
//! header. A nonce repeats under a key only when the header repeats — the same
//! chunk of the same block — and an honest sender's plaintext for that chunk is
//! then the same bytes, because a block's id commits to its transactions
//! (invariant 24) and its encoding is deterministic. A repeated nonce with a
//! repeated plaintext reveals that the datagram was sent twice, which the
//! header already says.

use std::fmt;

use chacha20poly1305::aead::{AeadInPlace, KeyInit};
use chacha20poly1305::{Key, Tag, XChaCha20Poly1305, XNonce};
use maya_ebpf_net_common::header::{HEADER_LEN, RelayHeader, TAG_LEN};
use zeroize::{ZeroizeOnDrop, Zeroizing};

use crate::error::RelayError;

/// Bytes each peer contributes to a relay key.
pub const CONTRIBUTION_LEN: usize = 32;

const KEY_CONTEXT: &str = "maya2c 2026-09-15 p2p block relay directional keys v1";
const KEY_ID_CONTEXT: &str = "maya2c 2026-09-15 p2p block relay key id v1";
const NONCE_CONTEXT: &str = "maya2c 2026-09-15 p2p block relay chunk nonce v1";

/// One direction's relay key.
///
/// Zeroized on drop. Its id is public — it travels in every header — and is a
/// one-way function of the key.
#[derive(ZeroizeOnDrop)]
pub struct RelayKey {
    key: [u8; 32],
    id: u64,
}

impl fmt::Debug for RelayKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RelayKey(id={:#018x}, <redacted>)", self.id)
    }
}

impl RelayKey {
    /// Wraps key material.
    #[must_use]
    pub fn from_bytes(key: &[u8; 32]) -> Self {
        let [a, b, c, d, e, f, g, h, ..] = blake3::derive_key(KEY_ID_CONTEXT, key);
        Self {
            key: *key,
            id: u64::from_be_bytes([a, b, c, d, e, f, g, h]),
        }
    }

    /// The id every header sealed under this key carries.
    #[must_use]
    pub const fn id(&self) -> u64 {
        self.id
    }

    /// Writes the whole datagram for `header` and `plaintext` into `out`.
    ///
    /// # Errors
    ///
    /// [`RelayError::WrongKey`] if the header names another key, or
    /// [`RelayError::PlaintextLength`] if `plaintext` is not the chunk the
    /// header describes.
    pub fn seal(
        &self,
        header: &RelayHeader,
        plaintext: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), RelayError> {
        self.check(header, plaintext.len())?;
        let aad = header.encode();
        out.clear();
        out.reserve(header.datagram_len());
        out.extend_from_slice(&aad);
        out.extend_from_slice(plaintext);
        let tag = self
            .cipher()
            .encrypt_in_place_detached(&chunk_nonce(&aad), &aad, &mut out[HEADER_LEN..])
            // Fails only for a message beyond the cipher's length limit, which
            // a 1,160-byte chunk is not.
            .map_err(|_| RelayError::PlaintextLength {
                actual: plaintext.len(),
                expected: header.plaintext_len(),
            })?;
        out.extend_from_slice(&tag);
        Ok(())
    }

    /// Authenticates and decrypts `sealed` into `out`.
    ///
    /// `out` must be exactly [`RelayHeader::plaintext_len`] bytes. On failure it
    /// is zeroed, so no unauthenticated plaintext survives.
    ///
    /// # Errors
    ///
    /// [`RelayError::Forged`] if authentication fails, or as [`RelayKey::seal`].
    pub fn open(
        &self,
        header: &RelayHeader,
        sealed: &[u8],
        out: &mut [u8],
    ) -> Result<(), RelayError> {
        self.check(header, out.len())?;
        let Some((ciphertext, tag)) = sealed.split_last_chunk::<TAG_LEN>() else {
            return Err(RelayError::Forged);
        };
        if ciphertext.len() != out.len() {
            return Err(RelayError::PlaintextLength {
                actual: ciphertext.len(),
                expected: out.len(),
            });
        }
        out.copy_from_slice(ciphertext);
        let aad = header.encode();
        let opened = self.cipher().decrypt_in_place_detached(
            &chunk_nonce(&aad),
            &aad,
            out,
            Tag::from_slice(tag),
        );
        if opened.is_err() {
            out.fill(0);
            return Err(RelayError::Forged);
        }
        Ok(())
    }

    fn check(&self, header: &RelayHeader, plaintext_len: usize) -> Result<(), RelayError> {
        if header.key_id() != self.id {
            return Err(RelayError::WrongKey {
                named: header.key_id(),
                held: self.id,
            });
        }
        if plaintext_len != header.plaintext_len() {
            return Err(RelayError::PlaintextLength {
                actual: plaintext_len,
                expected: header.plaintext_len(),
            });
        }
        Ok(())
    }

    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new(Key::from_slice(&self.key))
    }
}

fn chunk_nonce(header: &[u8; HEADER_LEN]) -> XNonce {
    let digest = blake3::Hasher::new_derive_key(NONCE_CONTEXT)
        .update(header)
        .finalize();
    XNonce::clone_from_slice(&digest.as_bytes()[..24])
}

/// Which side of the key exchange this node was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Sent the request.
    Requester,
    /// Answered it.
    Responder,
}

/// A peer pair's relay keys, from one side's point of view.
#[derive(Debug)]
pub struct DirectionalKeys {
    /// Seals what this node sends.
    pub outbound: RelayKey,
    /// Opens what this node receives.
    pub inbound: RelayKey,
}

/// Derives both directions' keys from the two contributions and peer ids.
///
/// Both sides call this with the same arguments except `role`, and each gets
/// the other's `inbound` as its `outbound`. The peer ids are length-prefixed so
/// no two pairs of ids hash alike.
#[must_use]
pub fn derive_keys(
    requester_contribution: &[u8; CONTRIBUTION_LEN],
    responder_contribution: &[u8; CONTRIBUTION_LEN],
    requester: &[u8],
    responder: &[u8],
    role: Role,
) -> DirectionalKeys {
    let mut hasher = blake3::Hasher::new_derive_key(KEY_CONTEXT);
    for id in [requester, responder] {
        hasher.update(&(id.len() as u64).to_be_bytes());
        hasher.update(id);
    }
    hasher.update(requester_contribution);
    hasher.update(responder_contribution);

    let mut material = Zeroizing::new([0u8; 64]);
    hasher.finalize_xof().fill(material.as_mut_slice());
    let mut forward = Zeroizing::new([0u8; 32]);
    let mut backward = Zeroizing::new([0u8; 32]);
    forward.copy_from_slice(&material[..32]);
    backward.copy_from_slice(&material[32..]);

    let requester_to_responder = RelayKey::from_bytes(&forward);
    let responder_to_requester = RelayKey::from_bytes(&backward);
    match role {
        Role::Requester => DirectionalKeys {
            outbound: requester_to_responder,
            inbound: responder_to_requester,
        },
        Role::Responder => DirectionalKeys {
            outbound: responder_to_requester,
            inbound: requester_to_responder,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> (DirectionalKeys, DirectionalKeys) {
        let a = [1u8; 32];
        let b = [2u8; 32];
        (
            derive_keys(&a, &b, b"alice", b"bob", Role::Requester),
            derive_keys(&a, &b, b"alice", b"bob", Role::Responder),
        )
    }

    #[test]
    fn each_side_opens_what_the_other_seals_and_the_directions_differ() {
        let (alice, bob) = pair();
        assert_eq!(alice.outbound.id(), bob.inbound.id());
        assert_eq!(bob.outbound.id(), alice.inbound.id());
        assert_ne!(alice.outbound.id(), alice.inbound.id());

        let header = RelayHeader::new(alice.outbound.id(), [9; 32], 0, 5).unwrap();
        let mut datagram = Vec::new();
        alice
            .outbound
            .seal(&header, b"hello", &mut datagram)
            .unwrap();
        assert_eq!(datagram.len(), header.datagram_len());

        let (parsed, sealed) = RelayHeader::split(&datagram).unwrap();
        let mut plain = [0u8; 5];
        bob.inbound.open(&parsed, sealed, &mut plain).unwrap();
        assert_eq!(&plain, b"hello");
    }

    #[test]
    fn a_flipped_bit_anywhere_fails_authentication_and_leaves_no_plaintext() {
        let (alice, bob) = pair();
        let header = RelayHeader::new(alice.outbound.id(), [9; 32], 0, 5).unwrap();
        let mut datagram = Vec::new();
        alice
            .outbound
            .seal(&header, b"hello", &mut datagram)
            .unwrap();
        for at in HEADER_LEN..datagram.len() {
            let mut tampered = datagram.clone();
            tampered[at] ^= 1;
            let (parsed, sealed) = RelayHeader::split(&tampered).unwrap();
            let mut plain = [0xFFu8; 5];
            assert!(matches!(
                bob.inbound.open(&parsed, sealed, &mut plain),
                Err(RelayError::Forged)
            ));
            assert_eq!(plain, [0; 5]);
        }
    }

    #[test]
    fn a_header_moved_onto_another_chunk_fails_authentication() {
        let (alice, bob) = pair();
        let first = RelayHeader::new(alice.outbound.id(), [9; 32], 0, 10).unwrap();
        let mut datagram = Vec::new();
        alice
            .outbound
            .seal(&first, b"0123456789", &mut datagram)
            .unwrap();
        // Same length, different block: only the AEAD's associated data differs.
        let other = RelayHeader::new(alice.outbound.id(), [8; 32], 0, 10).unwrap();
        let mut plain = [0u8; 10];
        assert!(matches!(
            bob.inbound
                .open(&other, &datagram[HEADER_LEN..], &mut plain),
            Err(RelayError::Forged)
        ));
    }

    #[test]
    fn different_contributions_or_peers_give_unrelated_keys() {
        let base = derive_keys(&[1; 32], &[2; 32], b"a", b"b", Role::Requester);
        let other_contribution = derive_keys(&[1; 32], &[3; 32], b"a", b"b", Role::Requester);
        let other_peer = derive_keys(&[1; 32], &[2; 32], b"a", b"c", Role::Requester);
        // Length prefixes: "ab" + "" must not collide with "a" + "b".
        let shifted = derive_keys(&[1; 32], &[2; 32], b"ab", b"", Role::Requester);
        let ids = [
            base.outbound.id(),
            other_contribution.outbound.id(),
            other_peer.outbound.id(),
            shifted.outbound.id(),
        ];
        for (i, a) in ids.iter().enumerate() {
            for b in &ids[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn a_key_refuses_a_header_naming_another_key() {
        let (alice, bob) = pair();
        let header = RelayHeader::new(alice.outbound.id(), [9; 32], 0, 5).unwrap();
        let mut out = Vec::new();
        assert!(matches!(
            bob.outbound.seal(&header, b"hello", &mut out),
            Err(RelayError::WrongKey { .. })
        ));
        assert!(matches!(
            alice.outbound.seal(&header, b"hell", &mut out),
            Err(RelayError::PlaintextLength { .. })
        ));
    }

    #[test]
    fn debug_output_never_shows_key_material() {
        let key = RelayKey::from_bytes(&[0xAB; 32]);
        let shown = format!("{key:?}");
        assert!(!shown.contains("ab, ab") && !shown.contains("171"));
    }
}
