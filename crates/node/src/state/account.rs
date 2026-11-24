//! Account records and their on-disk encoding.

use crate::error::{NodeError, Result};
use primitive_types::U256;

/// An account is addressed by the BLAKE3 hash of its ML-DSA-65 public key.
///
/// Under ed25519 this type *was* the public key. It is a hash now, because an
/// ML-DSA-65 key is 1952 bytes and putting that in every output, state key and
/// undo record was never an option. The width is unchanged, so everything keyed
/// by an address — including the VM host ABI — is untouched; what changed is
/// that an address no longer yields the key it commits to, so anything
/// verifying a signature must be handed that key separately.
pub type Address = [u8; 32];

/// Serialized length of an [`Account`]: a 32-byte little-endian `U256` balance,
/// a little-endian `u64` nonce, then the one-byte [`AUTH_KIND_KEY`]. ADR-046
/// widened the balance from 8 bytes; ADR-155 added the auth kind (P7).
pub const ACCOUNT_LEN: usize = 41;

/// Auth kind 0: the account spends with the key its address hashes, the only
/// kind this build executes (P7, ADR-155).
///
/// The byte is in every record and every state leaf from genesis, so a later
/// kind (a smart-account policy, a recovery set, a new key scheme) is a value
/// a scheduled upgrade defines, not a new record layout. A record naming any
/// other kind is refused at decode, never read as kind 0.
pub const AUTH_KIND_KEY: u8 = 0;

const AUTH_KIND_OFFSET: usize = 40;

/// Balance and replay counter for a single account.
///
/// An address that has never been touched is indistinguishable from one holding
/// [`Account::default`] — zero balance, zero nonce — so absent keys need no
/// special case.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Account {
    /// Spendable balance in base units (ADR-046: `U256`).
    pub balance: U256,
    /// Number of transactions this account has already sent. The next valid
    /// transaction must carry exactly this nonce.
    pub nonce: u64,
}

impl Account {
    /// Encodes the account as a fixed-width [`ACCOUNT_LEN`]-byte record.
    #[must_use]
    pub fn encode(&self) -> [u8; ACCOUNT_LEN] {
        let mut buf = [0u8; ACCOUNT_LEN];
        self.balance.to_little_endian(&mut buf[0..32]);
        buf[32..40].copy_from_slice(&self.nonce.to_le_bytes());
        buf[AUTH_KIND_OFFSET] = AUTH_KIND_KEY;
        buf
    }

    /// Decodes an account record.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::InvalidLength`] if `bytes` is not exactly
    /// [`ACCOUNT_LEN`] bytes, which indicates a corrupt or foreign record
    /// rather than a recoverable condition, and [`NodeError::Decode`] for an
    /// auth kind other than [`AUTH_KIND_KEY`].
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != ACCOUNT_LEN {
            return Err(NodeError::InvalidLength {
                what: "account record",
                expected: ACCOUNT_LEN,
                actual: bytes.len(),
            });
        }

        if bytes[AUTH_KIND_OFFSET] != AUTH_KIND_KEY {
            return Err(NodeError::Decode(format!(
                "account auth kind {} is not one this build executes",
                bytes[AUTH_KIND_OFFSET]
            )));
        }

        // Slices are exactly 32 and 8 bytes given the length check above.
        let balance = U256::from_little_endian(&bytes[0..32]);
        let mut nonce_bytes = [0u8; 8];
        nonce_bytes.copy_from_slice(&bytes[32..40]);

        Ok(Self {
            balance,
            nonce: u64::from_le_bytes(nonce_bytes),
        })
    }
}
