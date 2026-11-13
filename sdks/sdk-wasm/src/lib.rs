//! Browser-side signing and verification for `Maya2C`, via wasm-bindgen.
//!
//! # Why this crate does not use the node's `hybrid` module
//!
//! `custom_l1_node::crypto::hybrid` is the authoritative implementation, and
//! reaching for it would be the obvious right answer. It is not available:
//! `custom-l1-node` links `RocksDB`, which is C++, and C++ does not target
//! `wasm32-unknown-unknown`. There is no feature flag that removes it, because
//! the node *is* the database.
//!
//! So the construction is composed here from the same two primitives the node
//! composes it from — `fips204` for ML-DSA-65 and `maya-crypto-pq` for
//! SLH-DSA-SHA2-128s — both of which are pure Rust and both of which build for
//! wasm.
//!
//! **That is duplication of consensus-critical encoding.** Duplication is
//! dangerous when it is undetected, so it is detected: `tests/parity_tests.rs`
//! signs with the node's implementation and verifies with this one, and the
//! reverse, and compares addresses and encodings byte for byte. Drift fails a
//! test rather than producing a transaction the chain silently rejects.
//!
//! The structural fix is to extract `hybrid` into its own crate that both the
//! node and this SDK depend on. That is a refactor of a consensus path and is
//! deliberately not bundled into an SDK change; see `sdks/sdk-js/README.md`.
//!
//! # Cost in a browser
//!
//! A hybrid signature is 11,165 bytes and the hash-based half dominates
//! signing. In wasm there is no AVX2 and no SHA extensions, so expect
//! materially worse than the native figures. `sdk-js` measures it and reports;
//! nothing here asserts a bound. Sign off the main thread.

use fips204::ml_dsa_65;
use fips204::traits::{SerDes as _, Verifier as _};
use maya_crypto_pq::sig as slh;
use wasm_bindgen::prelude::*;

/// Bytes in an ML-DSA-65 public key.
pub const ML_DSA_PUBLIC_KEY_LEN: usize = 1952;
/// Bytes in an ML-DSA-65 signature.
pub const ML_DSA_SIGNATURE_LEN: usize = 3309;
/// Bytes in an SLH-DSA-SHA2-128s public key.
pub const SLH_DSA_PUBLIC_KEY_LEN: usize = slh::PUBLIC_KEY_LEN;
/// Bytes in an SLH-DSA-SHA2-128s signature.
pub const SLH_DSA_SIGNATURE_LEN: usize = slh::SIGNATURE_LEN;

/// Bytes in a hybrid public key: lattice then hash-based.
pub const HYBRID_PUBLIC_KEY_LEN: usize = ML_DSA_PUBLIC_KEY_LEN + SLH_DSA_PUBLIC_KEY_LEN;
/// Bytes in a hybrid signature: lattice then hash-based.
pub const HYBRID_SIGNATURE_LEN: usize = ML_DSA_SIGNATURE_LEN + SLH_DSA_SIGNATURE_LEN;

/// Address-derivation domain.
///
/// **Consensus.** This exact string, and this exact ordering, is what makes an
/// address derived in a browser the same address the chain credits. It is
/// copied from `custom_l1_node::crypto::hybrid`, and `parity_tests.rs` is what
/// keeps the copy honest.
const ADDRESS_DOMAIN: &[u8] = b"custom-l1-node.address.v3";

/// Derives the address a hybrid public key controls.
///
/// `blake3(domain ‖ ml_dsa_pk ‖ slh_dsa_pk)`, hex-encoded without a prefix.
///
/// # Why this is separate from the `#[wasm_bindgen]` export
///
/// `JsError` cannot be constructed outside a wasm target — it panics — so a
/// function returning one is untestable on the host. Every piece of logic here
/// therefore lives in a plain Rust function and the wasm layer is a wrapper
/// with no decisions in it.
///
/// That is not only for the unit tests: `tests/hybrid_parity_tests.rs` compares
/// this against the node's implementation on the host, and it could not run at
/// all if the logic were behind a JS type.
///
/// # Errors
///
/// Returns the reason if `public_key` is not [`HYBRID_PUBLIC_KEY_LEN`] bytes.
pub fn address_of(public_key: &[u8]) -> Result<String, String> {
    if public_key.len() != HYBRID_PUBLIC_KEY_LEN {
        return Err(format!(
            "public key must be {HYBRID_PUBLIC_KEY_LEN} bytes, got {}",
            public_key.len()
        ));
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update(ADDRESS_DOMAIN);
    hasher.update(public_key);
    Ok(hex::encode(hasher.finalize().as_bytes()))
}

/// [`address_of`], for JavaScript.
///
/// # Errors
///
/// Returns a `JsError` if `public_key` is the wrong length.
#[wasm_bindgen]
pub fn address_from_public_key(public_key: &[u8]) -> Result<String, JsError> {
    address_of(public_key).map_err(|e| JsError::new(&e))
}

/// Verifies the ML-DSA-65 half of a hybrid signature.
///
/// # This is not enough to accept a transaction
///
/// The chain requires **both** halves. A browser that checked only the lattice
/// proof would accept a transaction the chain rejects, and — more dangerously —
/// would display as valid a signature whose hash-based half was forged. This
/// exists because verifying 3,309 bytes is fast and verifying 11,165 is not,
/// so a UI can show a provisional result while [`verify_hybrid`] runs.
///
/// Anything that decides to *spend* must use [`verify_hybrid`].
#[wasm_bindgen]
pub fn verify_ml_dsa(public_key: &[u8], message: &[u8], signature: &[u8]) -> bool {
    let Ok(pk_bytes): Result<[u8; ML_DSA_PUBLIC_KEY_LEN], _> =
        public_key[..public_key.len().min(ML_DSA_PUBLIC_KEY_LEN)].try_into()
    else {
        return false;
    };
    let Ok(sig_bytes): Result<[u8; ML_DSA_SIGNATURE_LEN], _> =
        signature[..signature.len().min(ML_DSA_SIGNATURE_LEN)].try_into()
    else {
        return false;
    };
    let Ok(pk) = ml_dsa_65::PublicKey::try_from_bytes(pk_bytes) else {
        return false;
    };
    pk.verify(message, &sig_bytes, &[])
}

/// Verifies both halves of a hybrid signature.
///
/// Returns `true` only if the ML-DSA-65 proof *and* the SLH-DSA-SHA2-128s proof
/// verify over the same message. There is no mode in which one suffices.
#[wasm_bindgen]
pub fn verify_hybrid(public_key: &[u8], message: &[u8], signature: &[u8]) -> bool {
    if public_key.len() != HYBRID_PUBLIC_KEY_LEN || signature.len() != HYBRID_SIGNATURE_LEN {
        return false;
    }

    if !verify_ml_dsa(
        &public_key[..ML_DSA_PUBLIC_KEY_LEN],
        message,
        &signature[..ML_DSA_SIGNATURE_LEN],
    ) {
        return false;
    }

    let Ok(hash_pk): Result<[u8; SLH_DSA_PUBLIC_KEY_LEN], _> =
        public_key[ML_DSA_PUBLIC_KEY_LEN..].try_into()
    else {
        return false;
    };
    let Ok(hash_sig): Result<[u8; SLH_DSA_SIGNATURE_LEN], _> =
        signature[ML_DSA_SIGNATURE_LEN..].try_into()
    else {
        return false;
    };

    // Infallible: an SLH-DSA-SHA2-128s public key is 32 unstructured bytes,
    // so there is nothing to reject at parse time. A wrong key surfaces as a
    // failed verification below, which is the same answer one byte later.
    let verifying = slh::VerifyingKey::from_bytes(&hash_pk);
    verifying.verify(message, &hash_sig).is_ok()
}

pub mod wallet;

fn js_err(e: String) -> JsError {
    JsError::new(&e)
}

fn array32(bytes: &[u8], what: &str) -> Result<[u8; 32], JsError> {
    bytes
        .try_into()
        .map_err(|_| JsError::new(&format!("{what} must be 32 bytes, not {}", bytes.len())))
}

/// [`wallet::mnemonic_from_entropy`], for JavaScript.
///
/// # Errors
///
/// Returns a `JsError` for an entropy length BIP-39 does not allow.
#[wasm_bindgen(js_name = mnemonicFromEntropy)]
pub fn mnemonic_from_entropy(entropy: &[u8]) -> Result<String, JsError> {
    wallet::mnemonic_from_entropy(entropy)
        .map(|m| m.to_string())
        .map_err(js_err)
}

/// [`wallet::validate_mnemonic`], for JavaScript.
///
/// # Errors
///
/// Returns a `JsError` naming why the phrase is refused.
#[wasm_bindgen(js_name = validateMnemonic)]
pub fn validate_mnemonic(phrase: &str) -> Result<(), JsError> {
    wallet::validate_mnemonic(phrase).map_err(js_err)
}

/// A wallet seed held in wasm memory and wiped there when JavaScript calls
/// `free()` (or the object is collected): the 64 bytes never cross into
/// JavaScript, so there is no JS copy to forget and no freed-but-unwiped copy
/// left in linear memory by passing it back in on every call. The phrase and
/// passphrase arguments are still copied in by wasm-bindgen and freed
/// without a wipe (ADR-116, Consequences).
#[wasm_bindgen]
pub struct WalletSeed(zeroize::Zeroizing<[u8; wallet::SEED_LEN]>);

#[wasm_bindgen]
impl WalletSeed {
    /// [`wallet::seed_from_mnemonic`].
    ///
    /// # Errors
    ///
    /// Returns a `JsError` for an invalid phrase.
    #[wasm_bindgen(js_name = fromMnemonic)]
    pub fn from_mnemonic(phrase: &str, passphrase: &str) -> Result<WalletSeed, JsError> {
        wallet::seed_from_mnemonic(phrase, passphrase)
            .map(WalletSeed)
            .map_err(js_err)
    }

    /// [`wallet::public_key_at`].
    ///
    /// # Errors
    ///
    /// Returns a `JsError` for an index at or above 2^31.
    #[wasm_bindgen(js_name = publicKeyAt)]
    pub fn public_key_at(&self, account: u32, index: u32) -> Result<Vec<u8>, JsError> {
        wallet::public_key_at(self.0.as_ref(), account, index).map_err(js_err)
    }

    /// [`wallet::address_at`].
    ///
    /// # Errors
    ///
    /// Returns a `JsError` for an index at or above 2^31.
    #[wasm_bindgen(js_name = addressAt)]
    pub fn address_at(&self, account: u32, index: u32) -> Result<String, JsError> {
        wallet::address_at(self.0.as_ref(), account, index).map_err(js_err)
    }

    /// [`wallet::confirm_transfer`]. Amounts and `balance` are 32-byte
    /// little-endian; an empty `balance` means unknown.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` for a malformed argument, a fee to anyone but the
    /// collector, or a derivation failure.
    #[wasm_bindgen(js_name = confirmTransfer)]
    #[allow(clippy::too_many_arguments)]
    pub fn confirm_transfer(
        &self,
        account: u32,
        index: u32,
        recipient: &[u8],
        amount: &[u8],
        fee: &[u8],
        fee_recipient: &[u8],
        nonce: u64,
        chain: &[u8],
        balance: &[u8],
    ) -> Result<TransferConfirmation, JsError> {
        let transfer = transfer_of(recipient, amount, fee, fee_recipient, nonce)?;
        let balance = if balance.is_empty() {
            None
        } else {
            Some(array32(balance, "balance")?)
        };
        let c = wallet::confirm_transfer(
            self.0.as_ref(),
            account,
            index,
            &transfer,
            &array32(chain, "chain")?,
            balance.as_ref(),
        )
        .map_err(js_err)?;
        Ok(TransferConfirmation {
            signing_hash: c.signing_hash,
            description: c.description,
            recipient: c.recipient,
            sender: c.sender,
            address_prefix: c.address_prefix,
            warnings: c.warnings,
        })
    }

    /// [`wallet::sign_transfer`]. Amounts are 32-byte little-endian; `chain`
    /// is the 32-byte genesis id the signature commits to; `confirmed_hash`
    /// is the `signingHash` the confirm screen showed.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` for a malformed argument, a hash that names another
    /// transfer, or a signing failure.
    #[wasm_bindgen(js_name = signTransfer)]
    #[allow(clippy::too_many_arguments)]
    pub fn sign_transfer(
        &self,
        account: u32,
        index: u32,
        recipient: &[u8],
        amount: &[u8],
        fee: &[u8],
        fee_recipient: &[u8],
        nonce: u64,
        chain: &[u8],
        confirmed_hash: &str,
    ) -> Result<Vec<u8>, JsError> {
        let transfer = transfer_of(recipient, amount, fee, fee_recipient, nonce)?;
        wallet::sign_transfer(
            self.0.as_ref(),
            account,
            index,
            &transfer,
            &array32(chain, "chain")?,
            confirmed_hash,
        )
        .map_err(js_err)
    }
}

/// [`wallet::transfer_wire_len`], for JavaScript.
#[wasm_bindgen(js_name = transferWireLen)]
#[must_use]
pub fn transfer_wire_len(with_fee: bool) -> usize {
    wallet::transfer_wire_len(with_fee)
}

fn transfer_of(
    recipient: &[u8],
    amount: &[u8],
    fee: &[u8],
    fee_recipient: &[u8],
    nonce: u64,
) -> Result<wallet::Transfer, JsError> {
    Ok(wallet::Transfer {
        recipient: array32(recipient, "recipient")?,
        amount: array32(amount, "amount")?,
        fee: array32(fee, "fee")?,
        fee_recipient: array32(fee_recipient, "fee recipient")?,
        nonce,
    })
}

/// [`wallet::TransferConfirmation`], for JavaScript: plain fields, copied out.
#[wasm_bindgen(getter_with_clone)]
pub struct TransferConfirmation {
    /// Hex BLAKE3 of the signing bytes; hand it back to `signTransfer`.
    #[wasm_bindgen(js_name = signingHash)]
    pub signing_hash: String,
    /// The payment in plain words, recipient in full Bech32m.
    pub description: String,
    /// Recipient, grouped Bech32m.
    pub recipient: String,
    /// Sender, grouped Bech32m.
    pub sender: String,
    /// The address prefix the chain's genesis selects.
    #[wasm_bindgen(js_name = addressPrefix)]
    pub address_prefix: String,
    /// Warnings, most severe first.
    pub warnings: Vec<String>,
}

/// [`wallet::vault_key`], for JavaScript: Argon2id, desktop vault parameters.
///
/// # Errors
///
/// Returns a `JsError` for a salt Argon2 refuses.
#[wasm_bindgen(js_name = vaultKey)]
pub fn vault_key(password: &[u8], salt: &[u8]) -> Result<Vec<u8>, JsError> {
    wallet::vault_key(password, salt)
        .map(|k| k.to_vec())
        .map_err(js_err)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn the_lengths_match_the_chains() {
        // These are the numbers `docs/hybrid-signatures.md` quotes and the
        // chain enforces. A dependency bump that moved one would break the
        // encoding silently; this is where it stops.
        assert_eq!(HYBRID_PUBLIC_KEY_LEN, 1952 + SLH_DSA_PUBLIC_KEY_LEN);
        assert_eq!(HYBRID_SIGNATURE_LEN, 3309 + SLH_DSA_SIGNATURE_LEN);
        assert_eq!(HYBRID_SIGNATURE_LEN, 11_165);
    }

    #[test]
    fn a_wrong_length_public_key_is_refused_rather_than_truncated() {
        assert!(address_of(&[0u8; 10]).is_err());
        assert!(!verify_hybrid(
            &[0u8; 10],
            b"m",
            &[0u8; HYBRID_SIGNATURE_LEN]
        ));
        assert!(!verify_hybrid(
            &[0u8; HYBRID_PUBLIC_KEY_LEN],
            b"m",
            &[0u8; 10]
        ));
    }
}
