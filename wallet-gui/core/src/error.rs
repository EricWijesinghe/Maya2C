//! Wallet errors.
//!
//! Messages are written to be safe to show a user and safe to log. None of
//! them embeds key material, a mnemonic, or a passphrase — an error string is
//! the easiest place for a secret to escape a process.

use thiserror::Error;

/// Something the wallet could not do.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum WalletError {
    /// The OS entropy source failed.
    ///
    /// Fatal by design: generating a key from degraded randomness produces a
    /// wallet an attacker may be able to reproduce.
    #[error("entropy: {0}")]
    Entropy(String),

    /// A recovery phrase was rejected.
    #[error("recovery phrase: {0}")]
    Mnemonic(String),

    /// A derivation path was malformed or used a non-hardened index.
    #[error("derivation path: {0}")]
    InvalidPath(String),

    /// Key derivation failed.
    #[error("derivation: {0}")]
    Derivation(String),

    /// Producing a signature failed.
    ///
    /// Distinct from [`WalletError::Derivation`]: the key was fine and the
    /// signer refused. ML-DSA signing is a rejection-sampling loop that FIPS 204
    /// permits an implementation to give up on, so unlike ed25519 this is a
    /// fallible step rather than an infallible one.
    #[error("signing: {0}")]
    Signing(String),

    /// The OS keychain refused an operation.
    #[error("keychain: {0}")]
    Keychain(String),

    /// No wallet is stored under the requested name.
    #[error("no wallet named '{0}'")]
    NoWallet(String),

    /// A wallet already exists and would have been overwritten.
    ///
    /// Refusing is deliberate: silently replacing a stored seed destroys the
    /// only copy of a key with no way to recover it.
    #[error("a wallet named '{0}' already exists")]
    WalletExists(String),

    /// Decryption failed.
    ///
    /// A wrong passphrase and a tampered blob are deliberately indistinguishable
    /// — telling an attacker which one they got wrong is free information.
    #[error("could not unlock the wallet: wrong passphrase or corrupted data")]
    Unlock,

    /// The stored blob is not a wallet, or is a version this build cannot read.
    #[error("stored wallet is unreadable: {0}")]
    Format(String),

    /// A passphrase was empty where one is required.
    #[error("a passphrase is required")]
    EmptyPassphrase,

    /// A QR payload could not be decoded or parsed.
    #[error("qr: {0}")]
    Qr(String),

    /// An address was not 32 bytes of hex.
    #[error("address: {0}")]
    Address(String),

    /// A node rejected a transaction or could not be reached.
    #[error("node: {0}")]
    Node(String),

    /// Arithmetic on amounts overflowed.
    #[error("amount arithmetic overflowed")]
    AmountOverflow,
}

/// Convenience alias.
pub type Result<T> = core::result::Result<T, WalletError>;
