//! # Maya wallet core
//!
//! The trusted half of a zero-trust wallet. Everything that touches key
//! material lives here; the GUI is a rendering layer that never sees a secret.
//!
//! ## The trust boundary
//!
//! ```text
//!   Leptos WASM frontend          Rust core (this crate)
//!   ────────────────────          ──────────────────────
//!   addresses, amounts     <──    derived public data
//!   payload summaries      <──    what a signature would authorize
//!   signed raw hex         <──    sign_transfer()
//!
//!   passphrase             ──>    unlock, used and zeroized
//!   payment URIs           ──>    parsed HERE, never in JS
//!
//!   never crosses: seed, mnemonic, private key, chain code
//! ```
//!
//! A scanned QR code is attacker-controlled input. Parsing it in the frontend
//! would put that string inside the trust boundary, so [`payment`] decodes and
//! validates it and hands back a typed request or an error.
//!
//! ## Two layers over the key
//!
//! The seed is encrypted with a user passphrase *before* it reaches the OS
//! keychain. The keychain stops other users; the passphrase stops anything
//! running as this user — which on Windows is any process it owns. An attacker
//! holding the keychain blob still faces Argon2id at 64 MiB per guess.
//!
//! ## SLIP-0010, not BIP-32
//!
//! BIP-32 is defined over secp256k1 and does not carry to ed25519. See [`hd`]
//! — using it here would derive keys that look valid and that no other wallet
//! would reproduce from the same phrase.

#![warn(missing_docs)]

pub mod airgap;
pub mod compose;
pub mod error;
pub mod hd;
pub mod payment;
pub mod vault;
pub mod wallet;

pub use airgap::{Assembler, Frame, frame_count, split};
pub use compose::{SetupTrust, ShieldedComposer, SwapOutcome, compose_swap};
pub use error::{Result, WalletError};
pub use hd::{DerivationPath, generate_mnemonic, seed_from_mnemonic, validate_mnemonic};
pub use payment::{FeeTier, PaymentRequest, SignedTransfer, scan_payment_request, sign_transfer};
pub use vault::{MemoryStore, OsKeychain, SecretStore, Vault};
pub use wallet::{Account, PendingTransfer, Wallet};
