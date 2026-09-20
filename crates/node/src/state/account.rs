//! Account records and their on-disk encoding.

use crate::error::{NodeError, Result};

/// An account is addressed by the BLAKE3 hash of its ML-DSA-65 public key.
///
/// Under ed25519 this type *was* the public key. It is a hash now, because an
/// ML-DSA-65 key is 1952 bytes and putting that in every output, state key and
/// undo record was never an option. The width is unchanged, so everything keyed
/// by an address — including the VM host ABI — is untouched; what changed is
/// that an address no longer yields the key it commits to, so anything
/// verifying a signature must be handed that key separately.
pub type Address = [u8; 32];

/// Serialized length of an [`Account`]: two little-endian `u64`s.
pub const ACCOUNT_LEN: usize = 16;

/// Balance and replay counter for a single account.
///
/// An address that has never been touched is indistinguishable from one holding
/// [`Account::default`] — zero balance, zero nonce — so absent keys need no
/// special case.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Account {
    /// Spendable balance in base units.
    pub balance: u64,
    /// Number of transactions this account has already sent. The next valid
    /// transaction must carry exactly this nonce.
    pub nonce: u64,
}

impl Account {
    /// Encodes the account as a fixed-width 16-byte record.
    #[must_use]
    pub fn encode(&self) -> [u8; ACCOUNT_LEN] {
        let mut buf = [0u8; ACCOUNT_LEN];
        buf[0..8].copy_from_slice(&self.balance.to_le_bytes());
        buf[8..16].copy_from_slice(&self.nonce.to_le_bytes());
        buf
    }

    /// Decodes an account record.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::InvalidLength`] if `bytes` is not exactly
    /// [`ACCOUNT_LEN`] bytes, which indicates a corrupt or foreign record
    /// rather than a recoverable condition.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != ACCOUNT_LEN {
            return Err(NodeError::InvalidLength {
                what: "account record",
                expected: ACCOUNT_LEN,
                actual: bytes.len(),
            });
        }

        // Both slices are exactly 8 bytes given the length check above.
        let mut balance_bytes = [0u8; 8];
        balance_bytes.copy_from_slice(&bytes[0..8]);
        let mut nonce_bytes = [0u8; 8];
        nonce_bytes.copy_from_slice(&bytes[8..16]);

        Ok(Self {
            balance: u64::from_le_bytes(balance_bytes),
            nonce: u64::from_le_bytes(nonce_bytes),
        })
    }
}
