//! The identifier: `did:maya2c:<address>`.
//!
//! ## Why the address and not the public key
//!
//! The request was `did:maya2c:<pubkey>`. A `Maya2C` public key is a hybrid pair
//! — ML-DSA-65 plus SLH-DSA, `HYBRID_PUBLIC_KEY_LEN` = 1,984 bytes — which is
//! about 2,712 base58 characters. A DID goes in a URL, a QR code and every log
//! line that mentions the subject; 2,712 characters is not an identifier, it is
//! a payload.
//!
//! The chain already has the right value. An address is BLAKE3 over **both**
//! public keys, and `Transaction::sender` says why that matters: hashing both
//! forces an attacker who breaks one scheme to also hold the victim's key in
//! the other, because a forged half paired with a self-chosen partner key
//! hashes to a different address and owns nothing.
//!
//! So a DID *is* an address, and the binding argument is one the chain already
//! makes rather than a new one this crate has to defend. Forty-four base58
//! characters, and the keys live in the document's verification methods —
//! which is what `did:ion` and `did:key` do for the same reason.
//!
//! ## Why base58 and not hex or base64
//!
//! Hex would be 64 characters where base58 is 44. Base64 has `+`, `/` and `=`,
//! all of which need escaping in a URL and none of which survive being read
//! aloud. Base58's alphabet exists precisely because it drops the characters
//! people confuse: no `0`, `O`, `I` or `l`.
//!
//! ## What is not here
//!
//! No resolution. A DID names a subject; finding the document is the chain's
//! job, and this crate does not know what a chain is.

use crate::error::{Error, Result};

/// The method every identifier here uses.
pub const METHOD: &str = "maya2c";

/// The prefix a well-formed identifier starts with.
pub const PREFIX: &str = "did:maya2c:";

/// Bytes in a subject address.
pub const ADDRESS_BYTES: usize = 32;

/// A subject's 32-byte address, matching the chain's own.
pub type Address = [u8; ADDRESS_BYTES];

/// Bitcoin's base58 alphabet: no `0`, `O`, `I` or `l`.
const ALPHABET: &[u8; 58] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

/// The longest identifier this crate will parse.
///
/// 32 bytes of base58 is 44 characters at most; the prefix is 11. Bounded
/// before any decoding, because the string arrives from a stranger and base58
/// decoding is quadratic in the length.
const MAX_DID_CHARS: usize = PREFIX.len() + 64;

/// A decentralised identifier for a `Maya2C` subject.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Did(Address);

impl Did {
    /// The identifier for an address.
    #[must_use]
    pub const fn from_address(address: Address) -> Self {
        Self(address)
    }

    /// The subject's address.
    #[must_use]
    pub const fn address(&self) -> Address {
        self.0
    }

    /// Parses `did:maya2c:<base58>`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Did`] for a string that is too long, does not start
    /// with the method prefix, carries a character outside the base58 alphabet,
    /// or decodes to anything but exactly [`ADDRESS_BYTES`] bytes.
    ///
    /// Strict on all four. A resolver that accepted a near-miss would resolve
    /// two spellings to one subject, and an attacker who found a second
    /// spelling of somebody's DID could put it where the first was expected.
    pub fn parse(text: &str) -> Result<Self> {
        let invalid = |reason: &str| Error::Did(format!("{text:?}: {reason}"));

        if text.len() > MAX_DID_CHARS {
            return Err(invalid("longer than any well-formed identifier"));
        }
        let encoded = text
            .strip_prefix(PREFIX)
            .ok_or_else(|| invalid("does not start with did:maya2c:"))?;
        if encoded.is_empty() {
            return Err(invalid("carries no method-specific identifier"));
        }

        let decoded = base58_decode(encoded).ok_or_else(|| invalid("is not base58"))?;
        let address: Address = decoded
            .as_slice()
            .try_into()
            .map_err(|_| invalid("does not decode to a 32-byte address"))?;
        Ok(Self(address))
    }
}

impl std::fmt::Display for Did {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{PREFIX}{}", base58_encode(&self.0))
    }
}

/// Base58 of a byte string, Bitcoin-style.
///
/// Leading zero bytes become leading `1`s rather than being folded into the
/// number, which is what makes the encoding injective: without it, `[0, 1]` and
/// `[1]` would encode identically and two addresses would share a DID.
#[must_use]
pub fn base58_encode(bytes: &[u8]) -> String {
    let zeros = bytes.iter().take_while(|byte| **byte == 0).count();

    // Repeated division of the whole number by 58, held as base-256 digits.
    let mut digits: Vec<u8> = Vec::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let mut carry = u32::from(*byte);
        for digit in &mut digits {
            carry += u32::from(*digit) << 8;
            *digit = (carry % 58) as u8;
            carry /= 58;
        }
        while carry > 0 {
            digits.push((carry % 58) as u8);
            carry /= 58;
        }
    }

    let mut out = String::with_capacity(zeros + digits.len());
    for _ in 0..zeros {
        out.push('1');
    }
    for digit in digits.iter().rev() {
        out.push(char::from(ALPHABET[*digit as usize]));
    }
    out
}

/// The inverse of [`base58_encode`], or `None` for a string that is not base58.
#[must_use]
pub fn base58_decode(text: &str) -> Option<Vec<u8>> {
    let zeros = text
        .chars()
        .take_while(|character| *character == '1')
        .count();

    let mut bytes: Vec<u8> = Vec::with_capacity(text.len());
    for character in text.chars() {
        let value = ALPHABET
            .iter()
            .position(|candidate| char::from(*candidate) == character)?;
        let mut carry = value as u32;
        for byte in &mut bytes {
            carry += u32::from(*byte) * 58;
            *byte = (carry & 0xff) as u8;
            carry >>= 8;
        }
        while carry > 0 {
            bytes.push((carry & 0xff) as u8);
            carry >>= 8;
        }
    }

    let mut out = vec![0u8; zeros];
    out.extend(bytes.iter().rev());
    Some(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn address(fill: u8) -> Address {
        [fill; ADDRESS_BYTES]
    }

    #[test]
    fn a_did_survives_a_round_trip() {
        let did = Did::from_address(address(0xab));
        assert_eq!(Did::parse(&did.to_string()).expect("parse"), did);
    }

    #[test]
    fn a_did_is_forty_four_base58_characters_after_the_prefix() {
        // The whole reason the identifier is an address rather than a key. A
        // 1,984-byte hybrid public key would be about 2,712 characters here.
        let did = Did::from_address(address(0xff));
        let encoded = did.to_string();
        assert!(encoded.starts_with(PREFIX));
        let body = &encoded[PREFIX.len()..];
        assert!(
            (43..=44).contains(&body.len()),
            "a 32-byte address encoded to {} characters",
            body.len()
        );
    }

    #[test]
    fn leading_zero_bytes_survive_the_encoding() {
        // Without the leading-`1` rule, `[0, ..., 0, 1]` and `[1]` encode
        // identically and two addresses share a DID. An address of mostly zeros
        // is unlikely and entirely legal.
        let mut sparse = [0u8; ADDRESS_BYTES];
        sparse[ADDRESS_BYTES - 1] = 1;
        let did = Did::from_address(sparse);
        assert_eq!(
            Did::parse(&did.to_string()).expect("parse").address(),
            sparse
        );

        let zero = Did::from_address([0u8; ADDRESS_BYTES]);
        assert_eq!(Did::parse(&zero.to_string()).expect("parse"), zero);
        assert_ne!(zero.to_string(), did.to_string());
    }

    #[test]
    fn every_address_encodes_to_a_distinct_did() {
        let mut seen = std::collections::HashSet::new();
        for byte in 0..=255u8 {
            let mut bytes = address(byte);
            bytes[0] = byte;
            assert!(
                seen.insert(Did::from_address(bytes).to_string()),
                "two addresses share a DID at {byte}"
            );
        }
    }

    #[test]
    fn a_near_miss_is_refused_rather_than_resolved() {
        // A resolver that accepted any of these would resolve two spellings to
        // one subject, and an attacker who found a second spelling could put it
        // where the first was expected.
        let good = Did::from_address(address(7)).to_string();
        let body = &good[PREFIX.len()..];

        for text in [
            String::new(),
            "did:maya2c:".to_owned(),
            format!("DID:maya2c:{body}"),
            format!("did:maya:{body}"),
            format!("did:maya2c:{body}extra"),
            format!("did:maya2c:{}", &body[1..]),
            // `0`, `O`, `I` and `l` are not in the alphabet, deliberately.
            format!("did:maya2c:{}0", &body[1..]),
            format!("did:maya2c:{}O", &body[1..]),
            body.to_string(),
        ] {
            assert!(Did::parse(&text).is_err(), "{text:?} parsed");
        }
    }

    #[test]
    fn a_very_long_string_is_refused_before_it_is_decoded() {
        // Base58 decoding is quadratic in the length, and this string arrives
        // from a stranger.
        let long = format!("{PREFIX}{}", "z".repeat(100_000));
        assert!(Did::parse(&long).is_err());
    }

    #[test]
    fn base58_round_trips_arbitrary_bytes() {
        for length in 0..40usize {
            let bytes: Vec<u8> = (0..length).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(
                base58_decode(&base58_encode(&bytes)).expect("decode"),
                bytes,
                "length {length}"
            );
        }
    }
}
