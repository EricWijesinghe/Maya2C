//! The signed intent: "X of asset A for at most Y of asset B, by height D".

use maya_crypto_pq::suite::{
    HybridMlDsa65SlhDsa128s, HybridSigningKey, SignatureSuite, SuiteId, suite_address,
};
use primitive_types::U256;

use crate::error::{IntentError, Result};

/// What the signature commits to: this domain, then [`Intent::encode`].
const SIGN_DOMAIN: &[u8] = b"maya2c 2026-11-12 intent-solver intent v1";
/// The id is a keyed hash under its own context, never the signed bytes.
const ID_DOMAIN: &str = "maya2c 2026-11-12 intent-solver intent id v1";
/// Longest recipient on another chain (a bech32 or hex address fits).
pub const MAX_RECIPIENT_BYTES: usize = 64;
/// A `Maya2C` account is 32 bytes.
const NATIVE_RECIPIENT_BYTES: usize = 32;

/// An asset. [`AssetId::NATIVE`] is the coin the chain's HTLC form escrows;
/// any other id names an asset on another chain, by the caller's convention.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssetId(pub [u8; 32]);

impl AssetId {
    /// The native coin: the only asset `HtlcLock` escrows on `Maya2C`.
    pub const NATIVE: Self = Self([0; 32]);

    /// Whether the leg paying this asset settles on `Maya2C` itself.
    #[must_use]
    pub fn is_native(self) -> bool {
        self == Self::NATIVE
    }
}

/// An intent id: a hash of the intent's fields.
pub type IntentId = [u8; 32];

/// What a maker wants. Heights, never timestamps (invariant 9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Intent {
    /// Genesis-derived chain tag, so an intent cannot be replayed on another
    /// network.
    pub network: [u8; 32],
    /// The maker's account: [`Intent::maker_of`] its hybrid public key.
    pub maker: [u8; 32],
    /// What the maker gives.
    pub sell_asset: AssetId,
    /// The most of `sell_asset` the maker gives, fee included.
    pub max_sell: U256,
    /// What the maker wants.
    pub buy_asset: AssetId,
    /// Exactly how much of `buy_asset`.
    pub buy_amount: U256,
    /// Where `buy_asset` is paid: a 32-byte account for the native coin, the
    /// other chain's address bytes otherwise.
    pub buy_recipient: Vec<u8>,
    /// First height at which the intent is dead, as an HTLC expiry is.
    pub deadline_height: u64,
    /// Spent once per maker, whatever the rest of the intent says.
    pub nonce: u64,
}

impl Intent {
    /// The maker account a hybrid public key controls (suite `0x30`).
    #[must_use]
    pub fn maker_of(public_key: &[u8]) -> [u8; 32] {
        suite_address(SuiteId::HybridMlDsa65SlhDsa128s, public_key)
    }

    /// Canonical bytes: fixed-width fields, amounts big-endian, the recipient
    /// length-prefixed so no two intents share an encoding.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(32 * 6 + 1 + self.buy_recipient.len() + 16);
        out.extend_from_slice(&self.network);
        out.extend_from_slice(&self.maker);
        out.extend_from_slice(&self.sell_asset.0);
        out.extend_from_slice(&u256_bytes(self.max_sell));
        out.extend_from_slice(&self.buy_asset.0);
        out.extend_from_slice(&u256_bytes(self.buy_amount));
        // Bounded by `check` to 64, so the length fits a byte. Encoding an
        // unchecked intent only produces bytes no verifier accepts.
        out.push(u8::try_from(self.buy_recipient.len()).unwrap_or(u8::MAX));
        out.extend_from_slice(&self.buy_recipient);
        out.extend_from_slice(&self.deadline_height.to_le_bytes());
        out.extend_from_slice(&self.nonce.to_le_bytes());
        out
    }

    /// The id: BLAKE3 derive-key over [`Intent::encode`].
    #[must_use]
    pub fn id(&self) -> IntentId {
        let mut hasher = blake3::Hasher::new_derive_key(ID_DOMAIN);
        hasher.update(&self.encode());
        *hasher.finalize().as_bytes()
    }

    fn signing_message(&self) -> Vec<u8> {
        let mut message = SIGN_DOMAIN.to_vec();
        message.extend_from_slice(&self.encode());
        message
    }

    /// Field rules that need no key.
    fn check(&self) -> Result<()> {
        if self.sell_asset == self.buy_asset {
            return Err(IntentError::Malformed("sell and buy assets are the same"));
        }
        if self.max_sell.is_zero() || self.buy_amount.is_zero() {
            return Err(IntentError::Malformed("amounts must be non-zero"));
        }
        let len = self.buy_recipient.len();
        if len == 0 || len > MAX_RECIPIENT_BYTES {
            return Err(IntentError::Malformed("recipient length out of range"));
        }
        if self.buy_asset.is_native() && len != NATIVE_RECIPIENT_BYTES {
            return Err(IntentError::Malformed("a native recipient is 32 bytes"));
        }
        Ok(())
    }
}

/// An intent with the maker's hybrid key and signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedIntent {
    /// What was signed.
    pub intent: Intent,
    /// `ml_dsa_65_pk ‖ slh_dsa_sha2_128s_pk`.
    pub public_key: Vec<u8>,
    /// `ml_dsa_65_sig ‖ slh_dsa_sha2_128s_sig`; both halves must verify.
    pub signature: Vec<u8>,
}

impl SignedIntent {
    /// Signs `intent` with the maker's hybrid key.
    ///
    /// # Errors
    ///
    /// [`IntentError::BadSignature`] if the ML-DSA half could not sign.
    pub fn sign(intent: Intent, key: &HybridSigningKey) -> Result<Self> {
        let signature = HybridMlDsa65SlhDsa128s::sign(key, &intent.signing_message())
            .map_err(|_| IntentError::BadSignature)?;
        Ok(Self {
            public_key: HybridMlDsa65SlhDsa128s::public_key(key),
            intent,
            signature,
        })
    }

    /// The intent's id.
    #[must_use]
    pub fn id(&self) -> IntentId {
        self.intent.id()
    }

    /// Checks fields, network, key ownership, then the signature: the cheap
    /// refusals first, since verifying costs both signature schemes.
    ///
    /// # Errors
    ///
    /// [`IntentError::Malformed`], [`IntentError::WrongNetwork`],
    /// [`IntentError::MakerMismatch`] or [`IntentError::BadSignature`].
    pub fn verify(&self, network: &[u8; 32]) -> Result<()> {
        self.intent.check()?;
        if &self.intent.network != network {
            return Err(IntentError::WrongNetwork);
        }
        if Intent::maker_of(&self.public_key) != self.intent.maker {
            return Err(IntentError::MakerMismatch);
        }
        HybridMlDsa65SlhDsa128s::verify(
            &self.public_key,
            &self.intent.signing_message(),
            &self.signature,
        )
        .map_err(|_| IntentError::BadSignature)
    }
}

fn u256_bytes(value: U256) -> [u8; 32] {
    let mut out = [0u8; 32];
    value.to_big_endian(&mut out);
    out
}
