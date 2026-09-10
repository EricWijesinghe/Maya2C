//! Derivation path parsing and address computation.
//!
//! # Two constants are duplicated from the node, deliberately
//!
//! `COIN_TYPE` (7331) comes from `wallet-gui/core/src/hd.rs:53`, and
//! `ADDRESS_DOMAIN` from `src/crypto/hybrid.rs:111`. Importing them would mean
//! depending on `custom-l1-node`, which pulls RocksDB's C++ into a Cortex-M
//! binary.
//!
//! Duplication of a constant that decides where funds land is exactly the kind
//! of thing that drifts silently, so `tests/parity_tests.rs` pins both against
//! the node's own values. That is the same arrangement `sdk-wasm` has with
//! `tests/hybrid_parity_tests.rs`, and for the same reason: an independent
//! implementation that disagrees is indistinguishable from a correct one until
//! somebody loses money.

use crate::apdu::ApduError;

/// Maya2C's SLIP-44 coin type.
pub const COIN_TYPE: u32 = 7331;

/// Domain separator for address derivation.
///
/// A different domain yields a well-formed address that **no key can spend
/// from**. There is no error path for getting this wrong — funds simply go
/// nowhere — which is why it is pinned by a test rather than reviewed by eye.
pub const ADDRESS_DOMAIN: &[u8] = b"custom-l1-node.address.v3";

/// Bit marking a hardened path component.
pub const HARDENED: u32 = 0x8000_0000;

/// Components in the path this app accepts.
pub const PATH_LEN: usize = 5;

/// An accepted derivation path: `m/44'/7331'/account'/0'/index'`.
///
/// A fixed shape rather than an arbitrary path. A device that derived anywhere
/// the host asked would sign with a key the user's other software cannot find,
/// and "the wallet shows a different address than the device" is not a failure
/// anybody debugs quickly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DerivationPath {
    /// Account index, without the hardened bit.
    pub account: u32,
    /// Address index, without the hardened bit.
    pub index: u32,
}

impl DerivationPath {
    /// The five components, hardened.
    #[must_use]
    pub const fn components(self) -> [u32; PATH_LEN] {
        [
            44 | HARDENED,
            COIN_TYPE | HARDENED,
            self.account | HARDENED,
            HARDENED, // change = 0'
            self.index | HARDENED,
        ]
    }

    /// Parses the wire form: a count byte then big-endian `u32` components.
    ///
    /// # Errors
    ///
    /// [`ApduError::BadDerivationPath`] unless the path is exactly five
    /// hardened components with the right purpose and coin type. Every
    /// component is checked, including the ones that are constant — a host that
    /// sent `m/44'/0'/…` would otherwise get a Bitcoin-path key back and an
    /// address the chain has never seen.
    pub fn parse(bytes: &[u8]) -> Result<Self, ApduError> {
        if bytes.len() != 1 + PATH_LEN * 4 || bytes[0] as usize != PATH_LEN {
            return Err(ApduError::BadDerivationPath);
        }

        let mut components = [0u32; PATH_LEN];
        for (slot, raw) in components.iter_mut().zip(bytes[1..].as_chunks::<4>().0) {
            *slot = u32::from_be_bytes(*raw);
        }

        // Every component hardened. An unhardened component in a signing path
        // means a leaked child key can be walked back to its siblings.
        if components.iter().any(|c| c & HARDENED == 0) {
            return Err(ApduError::BadDerivationPath);
        }
        if components[0] != (44 | HARDENED) || components[1] != (COIN_TYPE | HARDENED) {
            return Err(ApduError::BadDerivationPath);
        }
        if components[3] != HARDENED {
            return Err(ApduError::BadDerivationPath);
        }

        Ok(Self {
            account: components[2] & !HARDENED,
            index: components[4] & !HARDENED,
        })
    }
}

/// Derives the 32-byte address from an encoded hybrid public key.
///
/// Byte-identical to the node's `address_of` (`src/crypto/hybrid.rs:216`):
///
/// ```text
/// BLAKE3( ADDRESS_DOMAIN || lattice || hash_based )
/// ```
///
/// # Two details that are easy to get wrong, and were
///
/// The domain is **prefixed into a plain hasher**, not passed to
/// `new_derive_key`. Those produce different digests from the same inputs, and
/// the first draft of this function used the keyed form — a mistake that
/// creates addresses nothing can spend from and that no error path reports.
///
/// `public_key` is the **encoded** key: `HybridPublicKey::encode_into` is a bare
/// concatenation of `lattice` then `hash_based` with no length prefixes, so the
/// 1,984 encoded bytes are exactly the two fields the node hashes, in order.
#[must_use]
pub fn address_from_public_key(public_key: &[u8]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(ADDRESS_DOMAIN);
    hasher.update(public_key);
    *hasher.finalize().as_bytes()
}
