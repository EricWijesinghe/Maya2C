//! Suite `0x10`: ML-DSA-65 alone, as a v7 suite-tagged transaction carries it.
//!
//! # Why this, and not the hybrid
//!
//! A hybrid (`0x30`) signature needs an SLH-DSA half the device cannot
//! produce in its RAM (`sign.rs`, `docs/ledger-feasibility.md`). Suite `0x10`
//! is a complete signature on its own: ADR-007's envelope names the suite,
//! and a v7 transaction signed under `0x10` needs nothing else. What it does
//! not have is activation — v7 is dark until `SUITE_ENVELOPE_ACTIVATION_HEIGHT`
//! — so a device signature is valid bytes the chain will accept on the day the
//! envelope activates, and not before.
//!
//! # Byte-identical to the wallet
//!
//! The same path on the device and in `crypto-pq` must give the same key, the
//! same address and the same signature, or a device-signed transaction has a
//! different id from the wallet's. So every step mirrors
//! `maya_crypto_pq::suite::MlDsa65`:
//!
//! 1. `ξ = BLAKE3-derive-key(XI_DOMAIN, chain key)` — `suite::expand`;
//! 2. `KeyGen_internal(ξ)` — computed by [`crate::lowmem`], which equals
//!    `fips204` and the NIST ACVP vectors byte for byte
//!    (`tests/lowmem_tests.rs`), and so equals RustCrypto `ml-dsa`;
//! 3. deterministic signing, `rnd = 0³²`, empty context — `sign_deterministic`.
//!
//! `fips204` itself is not on the device path: it needs 140–160 KiB of stack
//! where a Ledger has 28–40 KiB of RAM in all (`tests/memory_tests.rs`).
//!
//! The domains are duplicated rather than imported (the node would pull
//! RocksDB into a Cortex-M build) and pinned by `tests/parity_tests.rs`
//! against a fixture the node itself checks.

use zeroize::Zeroizing;

use crate::lowmem;

/// The registry byte.
pub const SUITE_ID: u8 = 0x10;
/// ML-DSA-65 public key length.
pub const PUBLIC_KEY_LEN: usize = lowmem::PUBLIC_KEY_LEN;
/// ML-DSA-65 expanded secret key length.
pub const SECRET_KEY_LEN: usize = lowmem::SECRET_KEY_LEN;
/// ML-DSA-65 signature length.
pub const SIGNATURE_LEN: usize = lowmem::SIGNATURE_LEN;

/// `crypto-pq`'s `ξ` domain for suite `0x10`.
pub const XI_DOMAIN: &str = "maya2c 2026-09-21 suite 0x10 ml-dsa-65 xi v1";
/// The node's suite-tagged address domain (`crypto/suites.rs`).
pub const SUITE_ADDRESS_DOMAIN: &str = "maya2c 2026-09-21 suite-tagged account address v1";

/// Deterministic signing's `rnd` (FIPS 204 §3.4).
const DETERMINISTIC_RND: [u8; 32] = [0; 32];
/// The empty context, as every suite signs.
const CONTEXT: &[u8] = b"";

/// A public key and its zeroizing secret key.
pub type Keypair = ([u8; PUBLIC_KEY_LEN], Zeroizing<[u8; SECRET_KEY_LEN]>);

/// The key pair for a derived chain key. The secret half is zeroized on drop.
///
/// Returns ~6 KB by value: for host code and tests. The device writes into
/// buffers it already owns with [`keypair_into`], because on a 32 KiB stack a
/// second copy of both keys is not free.
#[must_use]
pub fn keypair_from_chain_key(chain_key: &[u8; 32]) -> Keypair {
    let mut public = [0u8; PUBLIC_KEY_LEN];
    let mut secret = Zeroizing::new([0u8; SECRET_KEY_LEN]);
    keypair_into(chain_key, &mut public, &mut secret);
    (public, secret)
}

/// [`keypair_from_chain_key`] into caller-owned buffers.
///
/// `#[inline(never)]` here and in `lowmem` keep each frame separate, so the
/// stack holds the deepest chain of them rather than all of them at once.
#[inline(never)]
pub fn keypair_into(
    chain_key: &[u8; 32],
    public: &mut [u8; PUBLIC_KEY_LEN],
    secret: &mut [u8; SECRET_KEY_LEN],
) {
    let mut xi = Zeroizing::new([0u8; 32]);
    blake3::Hasher::new_derive_key(XI_DOMAIN)
        .update(chain_key)
        .finalize_xof()
        .fill(xi.as_mut_slice());
    lowmem::keygen(&xi, public, secret);
}

/// The account a suite-`0x10` key controls: the node's `suite_address`.
#[must_use]
pub fn address(public_key: &[u8; PUBLIC_KEY_LEN]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_derive_key(SUITE_ADDRESS_DOMAIN);
    hasher.update(&[SUITE_ID]);
    hasher.update(public_key);
    *hasher.finalize().as_bytes()
}

/// Signs `message` deterministically.
///
/// # Errors
///
/// [`lowmem::Error`], which a correct implementation never returns.
pub fn sign(
    secret: &[u8; SECRET_KEY_LEN],
    message: &[u8],
) -> Result<[u8; SIGNATURE_LEN], lowmem::Error> {
    let mut signature = [0u8; SIGNATURE_LEN];
    sign_into(secret, message, &mut signature)?;
    Ok(signature)
}

/// [`sign`] into a caller-owned buffer.
///
/// # Errors
///
/// As [`sign`].
#[inline(never)]
pub fn sign_into(
    secret: &[u8; SECRET_KEY_LEN],
    message: &[u8],
    signature: &mut [u8; SIGNATURE_LEN],
) -> Result<(), lowmem::Error> {
    lowmem::sign(secret, message, CONTEXT, &DETERMINISTIC_RND, signature)
}
