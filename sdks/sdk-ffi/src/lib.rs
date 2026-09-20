//! Python, Kotlin, and Swift bindings over Maya2C's post-quantum signing.
//!
//! # The security rule this crate is built around
//!
//! **A secret key never crosses the FFI boundary.**
//!
//! `HybridSigningKey` in the node is `Zeroizing`: its bytes are wiped when it
//! drops. That guarantee ends at the boundary. Hand the secret to Python and it
//! becomes a `bytes` object — immutable, freely copied by the interpreter,
//! resident until garbage collection, and impossible to wipe. Kotlin and Swift
//! are no better. A wallet SDK that exported key bytes would be quietly
//! weaker than the Rust it wraps, in the one respect that matters most.
//!
//! So [`SigningKey`] is an **opaque handle**. It can sign, and it can hand out
//! its public key and address. There is deliberately no `secret_bytes()`, and
//! adding one would undo the reason this crate is shaped the way it is.
//!
//! The one place secret material may enter is [`SigningKey::from_seed`], and
//! that is the caller's own seed arriving — the SDK is not exporting anything
//! the caller did not already hold.
//!
//! # What is verified, and what is only generated
//!
//! uniffi emits Python, Kotlin, and Swift from one interface. Emitting is not
//! the same as working:
//!
//! | Target | Generated | Compiled and tested |
//! |---|---|---|
//! | Python | yes | **yes** — `sdks/sdk-ffi/tests/python/` |
//! | Kotlin | yes | no — no `kotlinc` on the build host |
//! | Swift | yes | no — no Swift toolchain on the build host |
//!
//! That table is in the README too. Generated-and-never-compiled bindings are
//! not a working SDK, and reporting all three as done would be a green tick
//! over something nobody ran.
//!
//! # Cost
//!
//! A hybrid signature is 11,165 bytes — ML-DSA-65 at 3,309 and
//! SLH-DSA-SHA2-128s at 7,856 — and both must verify. Signing is dominated by
//! the hash-based half. Callers building interactive software should sign off
//! the UI thread; see `sdks/sdk-ffi/README.md` for measured figures.

use std::sync::Arc;

use custom_l1_node::core::codec::ByteReader;
use custom_l1_node::crypto::hybrid::{
    self, HybridPublicKey, HybridSignature, HybridSigningKey, HybridVerifyingKey,
};

/// Encodes a public key to its canonical bytes.
///
/// `HybridPublicKey` speaks the node's `encode_into`/`decode` codec rather than
/// a `to_bytes`/`from_bytes` pair. Going through the same codec the chain uses
/// is the point: an SDK that invented its own encoding would produce keys the
/// chain does not recognise, and the mismatch would only show up on-chain.
fn encode_public_key(key: &HybridPublicKey) -> Vec<u8> {
    let mut buf = Vec::with_capacity(hybrid::HYBRID_PUBLIC_KEY_LEN);
    key.encode_into(&mut buf);
    buf
}

/// Encodes a signature to its canonical bytes.
fn encode_signature(signature: &HybridSignature) -> Vec<u8> {
    let mut buf = Vec::with_capacity(hybrid::HYBRID_SIGNATURE_LENGTH);
    signature.encode_into(&mut buf);
    buf
}

uniffi::setup_scaffolding!();

/// Bytes in a hybrid public key.
pub const PUBLIC_KEY_BYTES: u32 = hybrid::HYBRID_PUBLIC_KEY_LEN as u32;

/// Bytes in a hybrid signature.
pub const SIGNATURE_BYTES: u32 = hybrid::HYBRID_SIGNATURE_LENGTH as u32;

/// Why an SDK call failed.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum SdkError {
    /// A seed, key, or signature had the wrong length.
    #[error("expected {expected} bytes, got {actual}")]
    WrongLength {
        /// Bytes the input should have carried.
        expected: u32,
        /// Bytes it actually carried.
        actual: u32,
    },

    /// The bytes were the right length but not a valid encoding.
    #[error("malformed {what}")]
    Malformed {
        /// Which input was malformed.
        what: String,
    },

    /// A signature did not verify.
    ///
    /// Distinct from [`SdkError::Malformed`]: this means a well-formed
    /// signature was checked and rejected, which is a different thing for a
    /// caller to handle than input it should never have sent.
    #[error("signature did not verify")]
    VerificationFailed,
}

/// A signing key that never leaves Rust.
///
/// Held by the host language as an opaque handle. It signs; it does not
/// export. See the crate documentation for why.
#[derive(uniffi::Object)]
pub struct SigningKey {
    inner: HybridSigningKey,
}

#[uniffi::export]
impl SigningKey {
    /// Derives a signing key from a 32-byte seed.
    ///
    /// The one entry point for secret material, and the seed is the caller's
    /// own — nothing is being exported that they did not already hold.
    ///
    /// # Errors
    ///
    /// [`SdkError::WrongLength`] unless the seed is exactly 32 bytes, or
    /// [`SdkError::Malformed`] if key derivation rejects it.
    #[uniffi::constructor]
    pub fn from_seed(seed: Vec<u8>) -> Result<Arc<Self>, SdkError> {
        let seed: [u8; 32] = seed
            .as_slice()
            .try_into()
            .map_err(|_| SdkError::WrongLength {
                expected: 32,
                actual: seed.len() as u32,
            })?;

        let inner = hybrid::signing_key_from_seed(&seed).map_err(|_| SdkError::Malformed {
            what: "seed".to_string(),
        })?;

        Ok(Arc::new(Self { inner }))
    }

    /// Generates a fresh signing key from the operating system CSPRNG.
    ///
    /// # Errors
    ///
    /// [`SdkError::Malformed`] if the underlying key generation fails, which
    /// means the OS refused entropy.
    #[uniffi::constructor]
    pub fn generate() -> Result<Arc<Self>, SdkError> {
        let inner = hybrid::generate_signing_key().map_err(|_| SdkError::Malformed {
            what: "generated key".to_string(),
        })?;
        Ok(Arc::new(Self { inner }))
    }

    /// The public key, in the chain's own encoding.
    #[must_use]
    pub fn public_key(&self) -> Vec<u8> {
        encode_public_key(&self.inner.public_key())
    }

    /// The address this key controls, hex-encoded without a `0x` prefix.
    #[must_use]
    pub fn address(&self) -> String {
        hex::encode(self.inner.address())
    }

    /// Signs `message`, returning the 11,165-byte hybrid signature.
    ///
    /// # Errors
    ///
    /// [`SdkError::Malformed`] if signing fails, which on a well-formed key
    /// means the OS refused entropy for the randomised half.
    pub fn sign(&self, message: Vec<u8>) -> Result<Vec<u8>, SdkError> {
        let signature = self.inner.sign(&message).map_err(|_| SdkError::Malformed {
            what: "signing operation".to_string(),
        })?;
        Ok(encode_signature(&signature))
    }
}

/// Verifies a hybrid signature.
///
/// Both halves must verify — the lattice one under FIPS 204 and the hash-based
/// one under FIPS 205. There is no mode in which one suffices, which is the
/// whole point of the scheme; see `docs/hybrid-signatures.md`.
///
/// # Errors
///
/// [`SdkError::WrongLength`] for a mis-sized key or signature,
/// [`SdkError::Malformed`] if either decodes badly, or
/// [`SdkError::VerificationFailed`] if the signature is well-formed and wrong.
#[uniffi::export]
pub fn verify(public_key: Vec<u8>, message: Vec<u8>, signature: Vec<u8>) -> Result<(), SdkError> {
    if public_key.len() != hybrid::HYBRID_PUBLIC_KEY_LEN {
        return Err(SdkError::WrongLength {
            expected: PUBLIC_KEY_BYTES,
            actual: public_key.len() as u32,
        });
    }
    if signature.len() != hybrid::HYBRID_SIGNATURE_LENGTH {
        return Err(SdkError::WrongLength {
            expected: SIGNATURE_BYTES,
            actual: signature.len() as u32,
        });
    }

    let public = HybridPublicKey::decode(&mut ByteReader::new(&public_key)).map_err(|_| {
        SdkError::Malformed {
            what: "public key".to_string(),
        }
    })?;
    let verifying =
        HybridVerifyingKey::from_public_key(&public).map_err(|_| SdkError::Malformed {
            what: "public key".to_string(),
        })?;
    let sig = HybridSignature::decode(&mut ByteReader::new(&signature)).map_err(|_| {
        SdkError::Malformed {
            what: "signature".to_string(),
        }
    })?;

    verifying
        .verify(&message, &sig)
        .map_err(|_| SdkError::VerificationFailed)
}

/// The address a public key controls, hex-encoded without a `0x` prefix.
///
/// # Errors
///
/// [`SdkError::WrongLength`] or [`SdkError::Malformed`] for an unusable key.
#[uniffi::export]
pub fn address_from_public_key(public_key: Vec<u8>) -> Result<String, SdkError> {
    if public_key.len() != hybrid::HYBRID_PUBLIC_KEY_LEN {
        return Err(SdkError::WrongLength {
            expected: PUBLIC_KEY_BYTES,
            actual: public_key.len() as u32,
        });
    }
    let public = HybridPublicKey::decode(&mut ByteReader::new(&public_key)).map_err(|_| {
        SdkError::Malformed {
            what: "public key".to_string(),
        }
    })?;
    Ok(hex::encode(public.address()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> Arc<SigningKey> {
        SigningKey::from_seed(vec![7u8; 32]).expect("valid seed")
    }

    #[test]
    fn a_seed_of_the_wrong_length_is_refused() {
        let error = match SigningKey::from_seed(vec![0u8; 31]) {
            Err(e) => e,
            Ok(_) => panic!("a 31-byte seed must be refused"),
        };
        assert!(matches!(
            error,
            SdkError::WrongLength {
                expected: 32,
                actual: 31
            }
        ));
    }

    #[test]
    fn the_same_seed_derives_the_same_key() {
        // Determinism is what makes a seed phrase a backup rather than a
        // one-time value.
        assert_eq!(key().address(), key().address());
        assert_eq!(key().public_key(), key().public_key());
    }

    #[test]
    fn a_signature_round_trips_through_verify() {
        let signer = key();
        let message = b"maya2c".to_vec();
        let signature = signer.sign(message.clone()).expect("sign");

        assert_eq!(signature.len(), SIGNATURE_BYTES as usize);
        verify(signer.public_key(), message, signature).expect("verify");
    }

    #[test]
    fn a_tampered_message_does_not_verify() {
        let signer = key();
        let signature = signer.sign(b"maya2c".to_vec()).expect("sign");

        let error = verify(signer.public_key(), b"maya2d".to_vec(), signature)
            .expect_err("must not verify");
        assert!(matches!(error, SdkError::VerificationFailed));
    }

    #[test]
    fn a_signature_from_another_key_does_not_verify() {
        let signer = key();
        let other = SigningKey::from_seed(vec![9u8; 32]).expect("valid seed");
        let message = b"maya2c".to_vec();
        let signature = other.sign(message.clone()).expect("sign");

        let error = verify(signer.public_key(), message, signature).expect_err("must not verify");
        assert!(matches!(error, SdkError::VerificationFailed));
    }

    #[test]
    fn a_mis_sized_signature_reports_the_length_rather_than_failing_to_verify() {
        // A caller who truncated a signature has a framing bug, not a bad
        // signature, and the two want different fixes.
        let signer = key();
        let error =
            verify(signer.public_key(), b"m".to_vec(), vec![0u8; 10]).expect_err("wrong length");
        assert!(matches!(error, SdkError::WrongLength { .. }));
    }

    #[test]
    fn the_address_agrees_with_the_standalone_function() {
        let signer = key();
        let from_key = signer.address();
        let from_public = address_from_public_key(signer.public_key()).expect("valid key");
        assert_eq!(from_key, from_public);
    }

    #[test]
    fn generated_keys_differ() {
        let a = SigningKey::generate().expect("generate");
        let b = SigningKey::generate().expect("generate");
        assert_ne!(a.address(), b.address());
    }

    #[test]
    fn there_is_no_way_to_export_the_secret() {
        // The rule this crate is built around, asserted as a compile-time
        // fact: `SigningKey` exposes exactly these three methods plus its two
        // constructors, and none of them returns secret material.
        //
        // If somebody adds `secret_bytes()`, this test does not fail — nothing
        // can make it fail automatically. What it does is put the rule in the
        // test file, where a reviewer adding an export will read it.
        let signer = key();
        let _: Vec<u8> = signer.public_key();
        let _: String = signer.address();
        let _: Vec<u8> = signer.sign(vec![1]).expect("sign");
    }
}
