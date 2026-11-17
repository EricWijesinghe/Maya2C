//! Stealth addresses over ML-KEM-768 — ADR-102. RESEARCH, dark.
//!
//! A recipient publishes a meta-address `(A, ek)`: `A` is an ordinary suite
//! address (the key that spends), `ek` an ML-KEM-768 encapsulation key whose
//! decapsulation key is the *scan key*. A sender encapsulates to `ek`, derives
//! a tweak from the shared secret, and pays the one-time address
//! `P = BLAKE3-derive-key("maya2c/stealth/addr/v1", A ‖ tweak)`, announcing the
//! ciphertext and a one-byte view tag beside the payment.
//!
//! What this gives and what it does not, stated as the ADR states it:
//!
//! - **Receipt is unlinkable.** Two payments to one meta-address land on two
//!   unrelated-looking `P`; linking either to `A` needs the shared secret.
//! - **The spend links `P` to `A`.** FIPS 204 keys have no homomorphism to
//!   tweak, so there is no one-time *secret* key: a spend presents `spend_pk`
//!   and the tweak ([`StealthAuth`]) and the verifier re-derives `P`.
//! - **The view tag saves only the address derivation.** Nothing cheaper than a
//!   decapsulation depends on the shared secret, so scanning costs one
//!   ML-KEM-768 decapsulation per announcement (`benches/stealth_scan.rs`).
//!
//! Nothing in consensus calls this module. The node's
//! `STEALTH_ACTIVATION_HEIGHT` is `u64::MAX` until an ADR-082 schedule says
//! otherwise.

use secrecy::{ExposeSecret as _, ExposeSecretMut as _, SecretBox};
use sha3::Shake256;
use sha3::digest::{ExtendableOutput as _, Update as _};
use zeroize::Zeroizing;

use crate::kem::{self, CIPHERTEXT_LEN, ENCAPSULATION_KEY_LEN, EncapsulationKey};
use crate::suite::{self, SignatureSuite, SuiteError, SuiteId};

/// Domain of the 64-byte KDF output `k` (ADR-102, sending step 2).
pub const KDF_DOMAIN: &[u8] = b"maya2c/stealth/v1";

/// BLAKE3 derive-key context of the one-time address (sending step 4).
pub const ADDRESS_DOMAIN: &str = "maya2c/stealth/addr/v1";

/// Prefix of the message a stealth spend signs, so a stealth signature can
/// never be replayed as a plain suite signature over the same bytes.
pub const SPEND_DOMAIN: &[u8] = b"maya2c/stealth/spend/v1";

/// Encoded announcement: `ct (1088) ‖ tag (1)`.
pub const ANNOUNCEMENT_LEN: usize = CIPHERTEXT_LEN + 1;

/// Encoded meta-address: `A (32) ‖ ek (1184)`.
pub const META_ADDRESS_LEN: usize = 32 + ENCAPSULATION_KEY_LEN;

/// Length of the FIPS 203 seed `d ‖ z` a scan key is rebuilt from.
pub const SCAN_SEED_LEN: usize = 64;

/// Errors from stealth encoding, key handling and spend verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StealthError {
    /// An announcement was not exactly [`ANNOUNCEMENT_LEN`] bytes.
    #[error("stealth announcement must be {ANNOUNCEMENT_LEN} bytes, got {0}")]
    AnnouncementLength(usize),
    /// A meta-address was not exactly [`META_ADDRESS_LEN`] bytes.
    #[error("stealth meta-address must be {META_ADDRESS_LEN} bytes, got {0}")]
    MetaAddressLength(usize),
    /// The meta-address's `ek` fails the FIPS 203 modulus check.
    #[error("stealth meta-address carries a malformed ML-KEM-768 key")]
    MalformedEncapsulationKey,
    /// The OS entropy source failed while generating a scan seed.
    #[error("OS entropy source unavailable")]
    Entropy,
    /// The spend's suite operation failed (length, encoding or signature).
    #[error(transparent)]
    Suite(#[from] SuiteError),
}

/// The 32-byte tweak binding a one-time address to its spend address.
///
/// Secret until the output is spent: anyone holding it and `A` can confirm
/// that `P` belongs to `A`. Zeroized on drop, never `Clone` or printed.
pub struct Tweak(Zeroizing<[u8; 32]>);

impl Tweak {
    /// The raw tweak, for [`one_time_address`] and [`StealthAuth::sign`].
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl core::fmt::Debug for Tweak {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("stealth::Tweak(<redacted>)")
    }
}

/// `k = SHAKE256(KDF_DOMAIN ‖ ss ‖ ct)`, 64 bytes; returns `(k[0], k[32..64])`.
///
/// Public for the conformance vectors; every field is fixed-length, so the
/// concatenation needs no length prefixes.
#[must_use]
pub fn derive_tag_and_tweak(ss: &[u8; 32], ct: &[u8; CIPHERTEXT_LEN]) -> (u8, Tweak) {
    let mut k = Zeroizing::new([0u8; 64]);
    let mut xof = Shake256::default();
    xof.update(KDF_DOMAIN);
    xof.update(ss);
    xof.update(ct);
    xof.finalize_xof_into(k.as_mut());
    let mut tweak = Zeroizing::new([0u8; 32]);
    tweak.copy_from_slice(&k[32..]);
    (k[0], Tweak(tweak))
}

/// `P = BLAKE3-derive-key(ADDRESS_DOMAIN, A ‖ tweak)`.
#[must_use]
pub fn one_time_address(spend_address: &[u8; 32], tweak: &[u8; 32]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_derive_key(ADDRESS_DOMAIN);
    hasher.update(spend_address);
    hasher.update(tweak);
    *hasher.finalize().as_bytes()
}

/// The record a stealth payment carries: the KEM ciphertext and the view tag.
#[derive(Clone, PartialEq, Eq)]
pub struct StealthAnnouncement {
    ct: [u8; CIPHERTEXT_LEN],
    tag: u8,
}

impl StealthAnnouncement {
    /// The ML-KEM-768 ciphertext.
    #[must_use]
    pub fn ciphertext(&self) -> &[u8; CIPHERTEXT_LEN] {
        &self.ct
    }

    /// The view tag, `k[0]`.
    #[must_use]
    pub fn tag(&self) -> u8 {
        self.tag
    }

    /// `ct ‖ tag`.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; ANNOUNCEMENT_LEN] {
        let mut out = [0u8; ANNOUNCEMENT_LEN];
        out[..CIPHERTEXT_LEN].copy_from_slice(&self.ct);
        out[CIPHERTEXT_LEN] = self.tag;
        out
    }

    /// Decodes `ct ‖ tag`.
    ///
    /// Length is the only structural check there is: every 1088-byte string
    /// is a ciphertext to ML-KEM (implicit rejection) and every byte a tag.
    ///
    /// # Errors
    ///
    /// [`StealthError::AnnouncementLength`] unless exactly [`ANNOUNCEMENT_LEN`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, StealthError> {
        if bytes.len() != ANNOUNCEMENT_LEN {
            return Err(StealthError::AnnouncementLength(bytes.len()));
        }
        let mut ct = [0u8; CIPHERTEXT_LEN];
        ct.copy_from_slice(&bytes[..CIPHERTEXT_LEN]);
        Ok(Self {
            ct,
            tag: bytes[CIPHERTEXT_LEN],
        })
    }
}

impl core::fmt::Debug for StealthAnnouncement {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "StealthAnnouncement(tag {:#04x}, ct ", self.tag)?;
        for byte in &self.ct[..8] {
            write!(f, "{byte:02x}")?;
        }
        f.write_str("…)")
    }
}

/// What a sender produces: the address to pay and the record to publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StealthPayment {
    /// The one-time address `P` the payment's output names.
    pub one_time_address: [u8; 32],
    /// The announcement carried in the same transaction.
    pub announcement: StealthAnnouncement,
}

/// A recipient's published stealth meta-address `(A, ek)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StealthMetaAddress {
    spend_address: [u8; 32],
    ek: EncapsulationKey,
}

impl StealthMetaAddress {
    /// The spend address `A`.
    #[must_use]
    pub fn spend_address(&self) -> &[u8; 32] {
        &self.spend_address
    }

    /// `A ‖ ek`.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(META_ADDRESS_LEN);
        out.extend_from_slice(&self.spend_address);
        out.extend_from_slice(&self.ek.to_bytes());
        out
    }

    /// Decodes `A ‖ ek`, running the FIPS 203 modulus check on `ek`.
    ///
    /// # Errors
    ///
    /// [`StealthError::MetaAddressLength`] or
    /// [`StealthError::MalformedEncapsulationKey`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, StealthError> {
        if bytes.len() != META_ADDRESS_LEN {
            return Err(StealthError::MetaAddressLength(bytes.len()));
        }
        let mut spend_address = [0u8; 32];
        spend_address.copy_from_slice(&bytes[..32]);
        let mut ek = [0u8; ENCAPSULATION_KEY_LEN];
        ek.copy_from_slice(&bytes[32..]);
        let ek = EncapsulationKey::from_bytes(&ek)
            .map_err(|_| StealthError::MalformedEncapsulationKey)?;
        Ok(Self { spend_address, ek })
    }

    /// Sender side: a fresh one-time address and its announcement.
    ///
    /// The shared secret and the tweak are dropped (zeroized) before return:
    /// the sender keeps nothing that could spend or later re-link the payment.
    #[must_use]
    pub fn pay(&self) -> StealthPayment {
        let (ct, ss) = self.ek.encapsulate();
        let (tag, tweak) = derive_tag_and_tweak(ss.as_bytes(), &ct);
        StealthPayment {
            one_time_address: one_time_address(&self.spend_address, tweak.as_bytes()),
            announcement: StealthAnnouncement { ct, tag },
        }
    }
}

/// The 64-byte seed a scan key is rebuilt from — what a wallet backs up.
pub struct ScanSeed(SecretBox<[u8; SCAN_SEED_LEN]>);

impl ScanSeed {
    /// Wraps existing seed bytes. The caller's copy is the caller's to wipe.
    #[must_use]
    pub fn from_bytes(bytes: [u8; SCAN_SEED_LEN]) -> Self {
        Self(SecretBox::new(Box::new(bytes)))
    }

    /// Draws a fresh seed from the operating system.
    ///
    /// # Errors
    ///
    /// [`StealthError::Entropy`] if the OS generator fails.
    pub fn generate() -> Result<Self, StealthError> {
        let mut seed = SecretBox::new(Box::new([0u8; SCAN_SEED_LEN]));
        getrandom::fill(seed.expose_secret_mut()).map_err(|_| StealthError::Entropy)?;
        Ok(Self(seed))
    }

    /// The seed bytes, for writing to an encrypted backup.
    #[must_use]
    pub fn expose(&self) -> &[u8; SCAN_SEED_LEN] {
        self.0.expose_secret()
    }
}

impl core::fmt::Debug for ScanSeed {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("stealth::ScanSeed(<redacted>)")
    }
}

/// A payment the view key recognised.
#[derive(Debug)]
pub struct Detected {
    /// Index of the matching output in the slice passed to [`ViewKey::scan`].
    pub output_index: usize,
    /// The one-time address `P`.
    pub one_time_address: [u8; 32],
    /// The tweak a spend must present ([`StealthAuth::sign`]).
    pub tweak: Tweak,
}

/// The wallet's view key: the ML-KEM-768 scan key plus the spend address `A`.
///
/// Detects payments and derives their addresses and tweaks; cannot spend.
/// May be handed to a scanning service (BIP-352's delegation model). The
/// decapsulation key zeroizes on drop (`ml-kem`'s `zeroize` feature).
pub struct ViewKey {
    dk: kem::DecapsulationKey,
    ek: EncapsulationKey,
    spend_address: [u8; 32],
}

impl core::fmt::Debug for ViewKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("stealth::ViewKey(<redacted>)")
    }
}

impl ViewKey {
    /// Rebuilds the view key for spend address `A` from its scan seed.
    #[must_use]
    pub fn from_seed(seed: &ScanSeed, spend_address: [u8; 32]) -> Self {
        let (dk, ek) = kem::DecapsulationKey::from_seed(seed.expose());
        Self {
            dk,
            ek,
            spend_address,
        }
    }

    /// The meta-address to publish.
    #[must_use]
    pub fn meta_address(&self) -> StealthMetaAddress {
        StealthMetaAddress {
            spend_address: self.spend_address,
            ek: self.ek.clone(),
        }
    }

    /// Checks one announcement against the outputs of its transaction.
    ///
    /// One decapsulation, always (implicit rejection: a foreign or malformed
    /// ciphertext yields a random secret, never an error). A tag mismatch stops
    /// there; a tag match (1 in 256 by chance) costs one BLAKE3 and a search,
    /// and only an output equal to `P` is a detection — so a tag collision is
    /// never a false acceptance.
    #[must_use]
    pub fn scan(
        &self,
        announcement: &StealthAnnouncement,
        outputs: &[[u8; 32]],
    ) -> Option<Detected> {
        let ss = self.dk.decapsulate(&announcement.ct);
        let (tag, tweak) = derive_tag_and_tweak(ss.as_bytes(), &announcement.ct);
        if tag != announcement.tag {
            return None;
        }
        let p = one_time_address(&self.spend_address, tweak.as_bytes());
        let output_index = outputs.iter().position(|o| *o == p)?;
        Some(Detected {
            output_index,
            one_time_address: p,
            tweak,
        })
    }
}

/// `SPEND_DOMAIN ‖ tweak ‖ message` — what a stealth spend signs.
///
/// The tweak is inside the signed bytes, so a relay that swaps it (to move the
/// proof onto another `P` of the same `A`) breaks the signature.
#[must_use]
pub fn spend_message(tweak: &[u8; 32], message: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(SPEND_DOMAIN.len() + 32 + message.len());
    out.extend_from_slice(SPEND_DOMAIN);
    out.extend_from_slice(tweak);
    out.extend_from_slice(message);
    out
}

/// The authorization of a spend from a one-time address (ADR-102 `StealthAuth`).
///
/// `message` is whatever the transaction layer signs (chain-tagged signing
/// bytes, ADR-036). The tweak is public from here on: spending reveals `A`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StealthAuth {
    /// Suite of the spend key (invariant 29: a registry suite).
    pub suite: SuiteId,
    /// The spend public key; `A = suite_address(suite, spend_pk)`.
    pub spend_pk: Vec<u8>,
    /// The tweak of the output being spent.
    pub tweak: [u8; 32],
    /// Signature over [`spend_message`].
    pub signature: Vec<u8>,
}

impl StealthAuth {
    /// Signs a spend of the output whose tweak is `tweak` with the spend key.
    ///
    /// # Errors
    ///
    /// [`StealthError::Suite`] if the backend refuses to sign.
    pub fn sign<S: SignatureSuite>(
        key: &S::SigningKey,
        tweak: &Tweak,
        message: &[u8],
    ) -> Result<Self, StealthError> {
        let signature = S::sign(key, &spend_message(tweak.as_bytes(), message))?;
        Ok(Self {
            suite: S::ID,
            spend_pk: S::public_key(key),
            tweak: *tweak.as_bytes(),
            signature,
        })
    }

    /// The one-time address this authorization can spend from.
    #[must_use]
    pub fn one_time_address(&self) -> [u8; 32] {
        one_time_address(
            &suite::suite_address(self.suite, &self.spend_pk),
            &self.tweak,
        )
    }

    /// Verifies the signature and returns the `P` it authorizes.
    ///
    /// The caller must still check that every consumed input belongs to the
    /// returned `P`, and the node must check suite admissibility at the height.
    ///
    /// # Errors
    ///
    /// [`StealthError::Suite`] on a length, encoding or signature failure.
    pub fn verify(&self, message: &[u8]) -> Result<[u8; 32], StealthError> {
        suite::verify(
            self.suite,
            &self.spend_pk,
            &spend_message(&self.tweak, message),
            &self.signature,
        )?;
        Ok(self.one_time_address())
    }
}
