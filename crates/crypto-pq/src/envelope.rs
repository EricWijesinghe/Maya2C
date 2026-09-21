//! The suite-tagged signature envelope — ADR-007.
//!
//! ```text
//! version:u8 ‖ suite:u8 ‖ pk_len:u32le ‖ pk ‖ sig_len:u32le ‖ sig
//! ```
//!
//! Variable-length on the wire, exact-length per suite: both lengths are
//! checked against the registry row *before* anything is allocated for them,
//! so a hostile length prefix costs the decoder nothing.
//!
//! A pre-envelope hybrid signature (the node's v1 transaction encoding) is a
//! valid suite `0x30` envelope once the two-byte header and the lengths are
//! prepended — [`SignedEnvelope::from_legacy_hybrid`] — so history keeps
//! verifying after the switch.

use crate::suite::{self, SuiteError, SuiteId};

/// The only envelope version this build writes or reads.
pub const ENVELOPE_VERSION: u8 = 2;

/// Bytes before the public key: version, suite, and the key's length.
const HEADER_LEN: usize = 1 + 1 + 4;

/// Errors decoding or checking an envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EnvelopeError {
    /// Fewer bytes than the header or a declared length needs.
    #[error("envelope truncated")]
    Truncated,
    /// A version this build does not know.
    #[error("unsupported envelope version {0}")]
    Version(u8),
    /// The suite byte, a length, or the signature itself.
    #[error(transparent)]
    Suite(#[from] SuiteError),
    /// Bytes after the signature. An envelope has exactly one encoding.
    #[error("{0} trailing bytes after envelope")]
    TrailingBytes(usize),
}

/// A public key and a signature, tagged with the suite that relates them.
#[derive(Clone, PartialEq, Eq)]
pub struct SignedEnvelope {
    suite: SuiteId,
    public_key: Vec<u8>,
    signature: Vec<u8>,
}

impl core::fmt::Debug for SignedEnvelope {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SignedEnvelope")
            .field("suite", &self.suite)
            .field("public_key_len", &self.public_key.len())
            .field("signature_len", &self.signature.len())
            .finish()
    }
}

impl SignedEnvelope {
    /// Wraps a key and signature, checking both lengths against `suite`.
    ///
    /// # Errors
    ///
    /// A length error if either does not match the registry.
    pub fn new(
        suite: SuiteId,
        public_key: Vec<u8>,
        signature: Vec<u8>,
    ) -> Result<Self, SuiteError> {
        suite::check_public_key_len(suite, &public_key)?;
        suite::check_signature_len(suite, &signature)?;
        Ok(Self {
            suite,
            public_key,
            signature,
        })
    }

    /// Lifts a pre-envelope hybrid key and signature into suite `0x30`.
    ///
    /// # Errors
    ///
    /// A length error if they are not the hybrid's sizes.
    pub fn from_legacy_hybrid(public_key: &[u8], signature: &[u8]) -> Result<Self, SuiteError> {
        Self::new(
            SuiteId::HybridMlDsa65SlhDsa128s,
            public_key.to_vec(),
            signature.to_vec(),
        )
    }

    /// The suite.
    #[must_use]
    pub fn suite(&self) -> SuiteId {
        self.suite
    }

    /// The encoded public key.
    #[must_use]
    pub fn public_key(&self) -> &[u8] {
        &self.public_key
    }

    /// The encoded signature.
    #[must_use]
    pub fn signature(&self) -> &[u8] {
        &self.signature
    }

    /// Verifies the signature over `message`.
    ///
    /// # Errors
    ///
    /// [`SuiteError::Verification`] if it does not verify.
    pub fn verify(&self, message: &[u8]) -> Result<(), SuiteError> {
        suite::verify(self.suite, &self.public_key, message, &self.signature)
    }

    /// The canonical encoding.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out =
            Vec::with_capacity(HEADER_LEN + 4 + self.public_key.len() + self.signature.len());
        out.push(ENVELOPE_VERSION);
        out.push(self.suite.to_byte());
        out.extend_from_slice(&len_prefix(self.public_key.len()));
        out.extend_from_slice(&self.public_key);
        out.extend_from_slice(&len_prefix(self.signature.len()));
        out.extend_from_slice(&self.signature);
        out
    }

    /// Decodes one envelope that must occupy all of `bytes`.
    ///
    /// # Errors
    ///
    /// [`EnvelopeError::TrailingBytes`] if anything follows it, and as
    /// [`Self::decode_prefix`] otherwise.
    pub fn decode(bytes: &[u8]) -> Result<Self, EnvelopeError> {
        let (envelope, used) = Self::decode_prefix(bytes)?;
        match bytes.len() - used {
            0 => Ok(envelope),
            extra => Err(EnvelopeError::TrailingBytes(extra)),
        }
    }

    /// Decodes one envelope from the front of `bytes`, returning it and the
    /// number of bytes it occupied.
    ///
    /// # Errors
    ///
    /// Truncation, an unknown version or suite, or a length that does not
    /// match the suite.
    pub fn decode_prefix(bytes: &[u8]) -> Result<(Self, usize), EnvelopeError> {
        let header = bytes.get(..HEADER_LEN).ok_or(EnvelopeError::Truncated)?;
        if header[0] != ENVELOPE_VERSION {
            return Err(EnvelopeError::Version(header[0]));
        }
        let suite = SuiteId::try_from(header[1])?;
        let info = suite.info();

        let pk_len = read_len(&header[2..6]);
        if pk_len != info.public_key_len {
            return Err(SuiteError::PublicKeyLength {
                suite,
                expected: info.public_key_len,
                got: pk_len,
            }
            .into());
        }
        let pk_end = HEADER_LEN + pk_len;
        let sig_len_bytes = bytes
            .get(pk_end..pk_end + 4)
            .ok_or(EnvelopeError::Truncated)?;
        let sig_len = read_len(sig_len_bytes);
        if sig_len != info.signature_len {
            return Err(SuiteError::SignatureLength {
                suite,
                expected: info.signature_len,
                got: sig_len,
            }
            .into());
        }
        let sig_start = pk_end + 4;
        let end = sig_start + sig_len;
        let signature = bytes.get(sig_start..end).ok_or(EnvelopeError::Truncated)?;
        Ok((
            Self {
                suite,
                public_key: bytes[HEADER_LEN..pk_end].to_vec(),
                signature: signature.to_vec(),
            },
            end,
        ))
    }
}

/// Every registry length fits a `u32` by construction (the largest is
/// SHAKE-256f's 49,856-byte signature).
fn len_prefix(len: usize) -> [u8; 4] {
    u32::try_from(len)
        .expect("registry lengths fit in u32")
        .to_le_bytes()
}

fn read_len(bytes: &[u8]) -> usize {
    let array: [u8; 4] = bytes.try_into().expect("caller slices exactly four bytes");
    // Widening on every supported target; a 16-bit usize is not one.
    u32::from_le_bytes(array) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::suite::{Ed25519, HybridMlDsa65SlhDsa128s, MasterSeed, MlDsa87, SignatureSuite};

    fn envelope<S: SignatureSuite>(message: &[u8]) -> SignedEnvelope {
        let key = S::signing_key_from_seed(&MasterSeed::from_bytes([5; 32]));
        let sig = S::sign(&key, message).expect("sign");
        SignedEnvelope::new(S::ID, S::public_key(&key), sig).expect("envelope")
    }

    #[test]
    fn an_envelope_round_trips_and_verifies() {
        for env in [
            envelope::<Ed25519>(b"m"),
            envelope::<MlDsa87>(b"m"),
            envelope::<HybridMlDsa65SlhDsa128s>(b"m"),
        ] {
            let bytes = env.encode();
            let back = SignedEnvelope::decode(&bytes).expect("decode");
            assert_eq!(back, env);
            assert_eq!(back.verify(b"m"), Ok(()));
            assert!(back.verify(b"n").is_err());
        }
    }

    #[test]
    fn a_legacy_hybrid_signature_lifts_into_suite_0x30() {
        let env = envelope::<HybridMlDsa65SlhDsa128s>(b"legacy");
        let lifted =
            SignedEnvelope::from_legacy_hybrid(env.public_key(), env.signature()).expect("lift");
        assert_eq!(lifted.suite(), SuiteId::HybridMlDsa65SlhDsa128s);
        assert_eq!(lifted.verify(b"legacy"), Ok(()));
    }

    #[test]
    fn every_truncation_is_refused() {
        let bytes = envelope::<Ed25519>(b"m").encode();
        for cut in 0..bytes.len() {
            assert!(
                SignedEnvelope::decode(&bytes[..cut]).is_err(),
                "cut at {cut}"
            );
        }
    }

    #[test]
    fn trailing_bytes_are_refused() {
        let mut bytes = envelope::<Ed25519>(b"m").encode();
        bytes.push(0);
        assert_eq!(
            SignedEnvelope::decode(&bytes),
            Err(EnvelopeError::TrailingBytes(1))
        );
        let (_, used) = SignedEnvelope::decode_prefix(&bytes).expect("prefix");
        assert_eq!(used, bytes.len() - 1);
    }

    #[test]
    fn a_wrong_version_suite_or_length_is_refused() {
        let good = envelope::<Ed25519>(b"m").encode();

        let mut version = good.clone();
        version[0] = 1;
        assert_eq!(
            SignedEnvelope::decode(&version),
            Err(EnvelopeError::Version(1))
        );

        let mut suite = good.clone();
        suite[1] = 0x7f;
        assert_eq!(
            SignedEnvelope::decode(&suite),
            Err(EnvelopeError::Suite(SuiteError::UnknownSuite(0x7f)))
        );

        // An Ed25519 envelope relabelled as ML-DSA-87 has the wrong key length.
        let mut relabelled = good.clone();
        relabelled[1] = SuiteId::MlDsa87.to_byte();
        assert!(matches!(
            SignedEnvelope::decode(&relabelled),
            Err(EnvelopeError::Suite(SuiteError::PublicKeyLength { .. }))
        ));

        // A hostile length prefix is refused before any allocation.
        let mut huge = good;
        huge[2..6].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(SignedEnvelope::decode(&huge).is_err());
    }

    #[test]
    fn new_refuses_a_mismatched_length() {
        assert!(SignedEnvelope::new(SuiteId::Ed25519, vec![0; 31], vec![0; 64]).is_err());
        assert!(SignedEnvelope::new(SuiteId::Ed25519, vec![0; 32], vec![0; 65]).is_err());
    }
}
