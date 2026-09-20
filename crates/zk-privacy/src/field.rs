//! Conversions between chain bytes and field elements.
//!
//! ## Why 32-byte values are split
//!
//! A BLS12-381 scalar holds 255 bits, so a 32-byte address does not fit. The
//! tempting shortcut — reduce the address modulo `r` — is a correctness bug:
//! the map stops being injective, and two distinct addresses collapse to the
//! same field element. In a shielded pool that means two distinct notes with
//! the same commitment, so splitting into two 128-bit limbs is not a stylistic
//! choice.

use ark_ff::{BigInteger, PrimeField};
use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};

use ark_bls12_381::Fr;

use crate::error::{Result, ZkError};

/// Bytes in a serialized field element.
pub const FIELD_BYTES: usize = 32;

/// Splits a 32-byte value into two field elements of 128 bits each.
///
/// The low limb is the first 16 bytes, the high limb the last 16. Both fit in
/// the field with room to spare, and the pair round-trips exactly.
#[must_use]
pub fn bytes_to_limbs(bytes: &[u8; 32]) -> [Fr; 2] {
    let mut low = [0u8; 16];
    let mut high = [0u8; 16];
    low.copy_from_slice(&bytes[..16]);
    high.copy_from_slice(&bytes[16..]);
    [
        Fr::from(u128::from_le_bytes(low)),
        Fr::from(u128::from_le_bytes(high)),
    ]
}

/// Reassembles the 32 bytes that [`bytes_to_limbs`] split.
///
/// # Errors
///
/// Returns [`ZkError::LimbOutOfRange`] if either limb exceeds 128 bits, which
/// means it did not come from `bytes_to_limbs`.
pub fn limbs_to_bytes(limbs: &[Fr; 2]) -> Result<[u8; 32]> {
    let mut out = [0u8; 32];
    for (index, limb) in limbs.iter().enumerate() {
        let repr = limb.into_bigint().to_bytes_le();
        // A 128-bit limb must be zero above its first 16 bytes.
        if repr.iter().skip(16).any(|byte| *byte != 0) {
            return Err(ZkError::LimbOutOfRange { index });
        }
        let take = repr.len().min(16);
        out[index * 16..index * 16 + take].copy_from_slice(&repr[..take]);
    }
    Ok(out)
}

/// Serializes a field element to its canonical 32-byte encoding.
#[must_use]
pub fn fr_to_bytes(value: &Fr) -> [u8; FIELD_BYTES] {
    let mut out = [0u8; FIELD_BYTES];
    // Compressed serialization of a scalar is exactly the field size, so this
    // cannot fail or produce a short write.
    value
        .serialize_compressed(&mut out[..])
        .expect("field element serializes into its own width");
    out
}

/// Parses a field element from its canonical encoding.
///
/// Rejects non-canonical encodings — anything at or above the field modulus.
/// That matters on a consensus path: two encodings of one value would be two
/// distinct nullifiers for the same note.
///
/// # Errors
///
/// Returns [`ZkError::NonCanonicalField`] if the bytes are not a canonical
/// element.
pub fn fr_from_bytes(bytes: &[u8; FIELD_BYTES]) -> Result<Fr> {
    Fr::deserialize_compressed(&bytes[..]).map_err(|_| ZkError::NonCanonicalField)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn limbs_round_trip_every_byte_position() {
        // Walk a distinct value through each byte so a limb boundary mistake
        // cannot hide behind a symmetric pattern.
        for position in 0..32usize {
            let mut input = [0u8; 32];
            input[position] = 0xA5;
            let limbs = bytes_to_limbs(&input);
            assert_eq!(limbs_to_bytes(&limbs).expect("round trip"), input);
        }
    }

    #[test]
    fn limbs_round_trip_all_ones() {
        let input = [0xFFu8; 32];
        let limbs = bytes_to_limbs(&input);
        assert_eq!(limbs_to_bytes(&limbs).expect("round trip"), input);
    }

    #[test]
    fn distinct_addresses_stay_distinct() {
        // The property that reducing mod r would destroy.
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        a[31] = 0x01;
        b[31] = 0x02;
        assert_ne!(bytes_to_limbs(&a), bytes_to_limbs(&b));
    }

    #[test]
    fn field_elements_round_trip() {
        let value = Fr::from(123_456_789u64);
        assert_eq!(fr_from_bytes(&fr_to_bytes(&value)).expect("parse"), value);
    }

    #[test]
    fn rejects_a_non_canonical_field_encoding() {
        // All-ones exceeds the modulus.
        let bytes = [0xFFu8; FIELD_BYTES];
        assert!(matches!(
            fr_from_bytes(&bytes),
            Err(ZkError::NonCanonicalField)
        ));
    }

    #[test]
    fn rejects_an_oversized_limb() {
        let limbs = [Fr::from(1u64), Fr::from(u128::MAX) + Fr::from(1u64)];
        assert!(matches!(
            limbs_to_bytes(&limbs),
            Err(ZkError::LimbOutOfRange { index: 1 })
        ));
    }
}
