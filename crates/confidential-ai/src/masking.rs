//! Masks, seeds and seals: the key material of one aggregation round.
//!
//! Every pair of participants `i < j` agrees a secret by ML-KEM-768 (FIPS 203):
//! `i` encapsulates to `j`'s key, `j` decapsulates. From that secret come two
//! things, under separate BLAKE3 domains so neither reveals the other:
//!
//! - the **pair seed**, expanded into a mask that `i` adds and `j` subtracts,
//!   so the pair's masks cancel in the sum and hide both inputs individually;
//! - a **share key** per direction, sealing each participant's share of its
//!   self-mask seed to the other with ChaCha20-Poly1305.
//!
//! Everything is bound to the round number and both indices, so no mask and no
//! key is ever used in two rounds or two directions — which is also why a zero
//! nonce is safe.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use maya_crypto_pq::kem::SharedSecret;
use zeroize::Zeroizing;

use crate::error::{Error, Result};
use crate::random::Randomness;

const PAIR_SEED_DOMAIN: &str = "maya2c confidential-ai pair seed v1";
const SHARE_KEY_DOMAIN: &str = "maya2c confidential-ai share key v1";
const COMMITMENT_DOMAIN: &str = "maya2c confidential-ai seed commitment v1";
const MASK_DOMAIN: &str = "maya2c confidential-ai mask v1";

/// A 32-byte secret that is wiped when dropped.
pub type Secret = Zeroizing<[u8; 32]>;

fn derive(domain: &str, parts: &[&[u8]]) -> Secret {
    let mut hasher = blake3::Hasher::new_derive_key(domain);
    for part in parts {
        hasher.update(part);
    }
    Zeroizing::new(*hasher.finalize().as_bytes())
}

/// The mask seed a pair shares in `round`. Symmetric in the two indices.
#[must_use]
pub fn pair_seed(secret: &SharedSecret, round: u64, a: u16, b: u16) -> Secret {
    let (low, high) = if a < b { (a, b) } else { (b, a) };
    derive(
        PAIR_SEED_DOMAIN,
        &[
            secret.as_bytes(),
            &round.to_le_bytes(),
            &low.to_le_bytes(),
            &high.to_le_bytes(),
        ],
    )
}

/// The key sealing shares sent from `from` to `to` in `round`.
#[must_use]
pub fn share_key(secret: &SharedSecret, round: u64, from: u16, to: u16) -> Secret {
    derive(
        SHARE_KEY_DOMAIN,
        &[
            secret.as_bytes(),
            &round.to_le_bytes(),
            &from.to_le_bytes(),
            &to.to_le_bytes(),
        ],
    )
}

/// A hiding commitment to a self-mask seed, published before the seed is shared.
#[must_use]
pub fn seed_commitment(seed: &[u8; 32]) -> [u8; 32] {
    *derive(COMMITMENT_DOMAIN, &[seed])
}

/// A seed expanded into `dimension` uniform residues.
#[must_use]
pub fn expand(seed: &[u8; 32], dimension: usize) -> Vec<u32> {
    let mut stream = Randomness::from_seed(seed, MASK_DOMAIN);
    let mut word = [0u8; 4];
    (0..dimension)
        .map(|_| {
            stream.fill(&mut word);
            u32::from_le_bytes(word)
        })
        .collect()
}

/// `values += mask` or `values -= mask`, coordinate-wise modulo 2^32.
pub fn apply(values: &mut [u32], mask: &[u32], add: bool) {
    for (value, mask) in values.iter_mut().zip(mask) {
        *value = if add {
            value.wrapping_add(*mask)
        } else {
            value.wrapping_sub(*mask)
        };
    }
}

fn associated_data(round: u64, from: u16, to: u16) -> [u8; 12] {
    let mut aad = [0u8; 12];
    aad[..8].copy_from_slice(&round.to_le_bytes());
    aad[8..10].copy_from_slice(&from.to_le_bytes());
    aad[10..].copy_from_slice(&to.to_le_bytes());
    aad
}

/// Seals `plaintext` under a single-use key.
///
/// # Errors
///
/// [`Error::InvalidParameter`] if the cipher refuses the input length.
pub fn seal(key: &Secret, round: u64, from: u16, to: u16, plaintext: &[u8]) -> Result<Vec<u8>> {
    ChaCha20Poly1305::new(Key::from_slice(key.as_ref()))
        .encrypt(
            &Nonce::default(),
            Payload {
                msg: plaintext,
                aad: &associated_data(round, from, to),
            },
        )
        .map_err(|_| Error::InvalidParameter("plaintext too long to seal"))
}

/// Opens a sealed message.
///
/// # Errors
///
/// [`Error::AuthenticationFailed`] for a wrong key, a changed byte, or a
/// message sealed for another round or direction.
pub fn open(
    key: &Secret,
    round: u64,
    from: u16,
    to: u16,
    sealed: &[u8],
) -> Result<Zeroizing<Vec<u8>>> {
    ChaCha20Poly1305::new(Key::from_slice(key.as_ref()))
        .decrypt(
            &Nonce::default(),
            Payload {
                msg: sealed,
                aad: &associated_data(round, from, to),
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| Error::AuthenticationFailed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use maya_crypto_pq::kem::generate_keypair;

    #[test]
    fn both_ends_of_an_ml_kem_exchange_derive_the_same_pair_seed() {
        let (decapsulation, encapsulation) = generate_keypair();
        let (ciphertext, sender) = encapsulation.encapsulate();
        let receiver = decapsulation.decapsulate(&ciphertext);
        assert_eq!(*pair_seed(&sender, 3, 1, 7), *pair_seed(&receiver, 3, 7, 1));
        assert_ne!(*pair_seed(&sender, 3, 1, 7), *pair_seed(&sender, 4, 1, 7));
        assert_ne!(*share_key(&sender, 3, 1, 7), *share_key(&sender, 3, 7, 1));
    }

    #[test]
    fn masks_cancel_and_seals_refuse_any_change() {
        let mask = expand(&[1; 32], 5);
        let mut values = vec![10u32, 20, 30, 40, 50];
        apply(&mut values, &mask, true);
        apply(&mut values, &mask, false);
        assert_eq!(values, vec![10, 20, 30, 40, 50]);

        let key = Zeroizing::new([9u8; 32]);
        let mut sealed = seal(&key, 1, 2, 3, b"share").expect("seal");
        assert_eq!(
            open(&key, 1, 2, 3, &sealed).expect("open").as_slice(),
            b"share"
        );
        assert_eq!(
            open(&key, 1, 3, 2, &sealed),
            Err(Error::AuthenticationFailed)
        );
        sealed[0] ^= 1;
        assert_eq!(
            open(&key, 1, 2, 3, &sealed),
            Err(Error::AuthenticationFailed)
        );
    }
}
