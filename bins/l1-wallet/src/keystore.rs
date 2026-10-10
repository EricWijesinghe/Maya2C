//! Encrypted hybrid ML-DSA-65 + SLH-DSA-SHA2-128s keystore.
//!
//! ## Format
//!
//! ```text
//! magic   8 bytes   "L1WALLET"
//! version 1 byte    1, 2 = ed25519, 3 = ML-DSA-65 (all rejected),
//!                   4 = hybrid pair (current)
//! salt   16 bytes   Argon2id salt, random per keystore
//! nonce  12 bytes   ChaCha20-Poly1305 nonce, random per encryption
//! cipher N bytes    plaintext + 16-byte authentication tag
//! ```
//!
//! ## Versions
//!
//! Version 4 stores an ML-DSA-65 private key followed by an SLH-DSA-SHA2-128s
//! private key, 4096 bytes in total. Version 3 held the lattice key alone;
//! versions 1 and 2 held ed25519 keys, raw and PKCS#8 respectively.
//!
//! **Every earlier version is recognized but cannot be used.** Normally a
//! keystore reader must never refuse an older file, because the keystore is the
//! only copy of a key and refusing it destroys funds. That reasoning does not
//! apply across these boundaries: an ed25519 or lattice-only key authorizes
//! nothing on a chain that requires two signatures under addresses derived from
//! two keys, so there are no funds behind it to destroy.
//!
//! Version 3 deserves the sharper statement, because it is the one that looks
//! upgradable and is not. Its ML-DSA key is still a valid ML-DSA key — but an
//! address is `blake3(v3 ‖ ml_dsa_pk ‖ slh_dsa_pk)`, so pairing it with a fresh
//! SLH-DSA key produces a *different* address, holding nothing. There is no
//! migration to write. What matters is that the failure says so, rather than
//! surfacing as a wrong-password or corrupt-file error that sends the user
//! looking for a problem they do not have.
//!
//! Version 4 is not PKCS#8. Version 2 was, so other tooling could read the
//! plaintext as a standard structure; neither `fips204` nor `slh-dsa` offers a
//! private-key encoding to reuse for a *pair*, and the OIDs are not something
//! to invent locally. A bare fixed-length concatenation is honest about being
//! this wallet's own format, where a hand-rolled DER blob would look
//! interoperable without being so.
//!
//! ## Design notes
//!
//! The password is stretched with **Argon2id at 64 MiB** before it ever touches
//! the cipher. A raw password used directly as a key would fall to offline
//! brute force the moment the file is copied; the memory-hard KDF is what makes
//! a stolen keystore expensive rather than trivial to crack.
//!
//! **ChaCha20-Poly1305** is authenticated encryption, so tampering with the
//! ciphertext is detected at decryption rather than silently yielding a
//! different key. The nonce is random and freshly generated on every write —
//! reusing a nonce under the same key would be catastrophic for `ChaCha20`.
//!
//! The salt is stored in the clear, which is correct: a salt is not a secret,
//! it exists to make precomputed rainbow tables useless.
//!
//! Decrypted secret material is wrapped in [`Zeroizing`] so it is wiped from
//! memory on drop rather than lingering in a freed allocation.

use anyhow::{Context, Result, anyhow, bail};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use custom_l1_node::crypto::hybrid::{HYBRID_SECRET_KEY_LEN, HybridSigningKey};
use zeroize::Zeroizing;

/// File magic identifying a wallet keystore.
const MAGIC: &[u8; 8] = b"L1WALLET";

/// Legacy version: ciphertext is a bare 32-byte ed25519 seed.
const VERSION_ED25519_RAW: u8 = 1;

/// Legacy version: ciphertext is an ed25519 PKCS#8 `PrivateKeyInfo` DER.
const VERSION_ED25519_PKCS8: u8 = 2;

/// Legacy version: ciphertext is a bare ML-DSA-65 private key.
const VERSION_MLDSA: u8 = 3;

/// Current version: ciphertext is an ML-DSA-65 private key followed by an
/// SLH-DSA-SHA2-128s private key, 4096 bytes in total.
///
/// Not PKCS#8. The previous ed25519 version wrapped the key in a standard
/// structure so other tooling could read it; neither `fips204` nor `slh-dsa`
/// gives us a private-key encoding to reuse for a *pair*, and the OIDs are not
/// something to invent locally. A bare fixed-length concatenation is honest
/// about being this wallet's own format, where a hand-rolled DER blob would
/// look interoperable without being so.
///
/// A v3 keystore cannot be upgraded in place. It holds a lattice key and
/// nothing else, and the missing SLH-DSA half is not derivable from it — an
/// account is named by the hash of *both* keys, so inventing a second half
/// would produce a different address holding no funds.
const VERSION_HYBRID: u8 = 4;

const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
/// Poly1305 authentication tag length.
const TAG_LEN: usize = 16;

const HEADER_LEN: usize = 8 + 1 + SALT_LEN + NONCE_LEN;

/// Smallest possible file: header plus a tag and at least one plaintext byte.
const MIN_KEYSTORE_LEN: usize = HEADER_LEN + TAG_LEN + 1;

/// Upper bound on a keystore file.
///
/// A hybrid key pair is 4096 bytes — 4032 for ML-DSA-65 plus 64 for
/// SLH-DSA-SHA2-128s — so a keystore is roughly 4.2 KiB. The bound leaves
/// headroom while still refusing to run a 64 MiB KDF over an arbitrarily large
/// file handed to us as `wallet.key`.
///
/// Note how little the hash-based half costs here, against every other place it
/// shows up: its secret key is 64 bytes where its signature is 7856. SLH-DSA
/// keys are seeds, not structures.
const MAX_KEYSTORE_LEN: usize = 8_192;

/// Argon2id memory cost for password stretching, in KiB (64 MiB).
///
/// Higher than the 32 MiB used for proof of work: this runs once per unlock,
/// where a human is waiting, so the cost budget is far larger than for a hash
/// that runs millions of times.
const KDF_MEMORY_KIB: u32 = 64 * 1024;

/// Argon2id passes.
const KDF_ITERATIONS: u32 = 3;

/// Argon2id lanes.
const KDF_LANES: u32 = 1;

/// Derives the file encryption key from a password and salt.
fn derive_key(password: &str, salt: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
    let params = Params::new(KDF_MEMORY_KIB, KDF_ITERATIONS, KDF_LANES, Some(32))
        .map_err(|e| anyhow!("invalid Argon2 parameters: {e}"))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

    let mut key = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(password.as_bytes(), salt, &mut key[..])
        .map_err(|e| anyhow!("key derivation failed: {e}"))?;
    Ok(key)
}

/// Builds the AEAD cipher from a derived key.
fn build_cipher(key: &[u8]) -> Result<ChaCha20Poly1305> {
    ChaCha20Poly1305::new_from_slice(key)
        .map_err(|_| anyhow!("derived key has the wrong length for ChaCha20-Poly1305"))
}

/// Encrypts `signing_key` under `password`, returning the keystore bytes.
///
/// # Errors
///
/// Returns an error if key derivation or encryption fails.
pub fn encrypt(signing_key: &HybridSigningKey, password: &str) -> Result<Vec<u8>> {
    if password.is_empty() {
        bail!("refusing to encrypt with an empty password");
    }

    // Straight from the OS CSPRNG. A failure here must abort rather than fall
    // back: a predictable nonce under ChaCha20 leaks the keystream, and a
    // predictable salt defeats the KDF's whole purpose.
    let mut salt = [0u8; SALT_LEN];
    getrandom::fill(&mut salt).map_err(|e| anyhow!("OS entropy unavailable: {e}"))?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    getrandom::fill(&mut nonce_bytes).map_err(|e| anyhow!("OS entropy unavailable: {e}"))?;

    let key = derive_key(password, &salt)?;
    let cipher = build_cipher(&key[..])?;

    // `to_bytes` hands back a Zeroizing buffer, so the plaintext key does not
    // outlive this function in freed memory.
    let secret = signing_key.to_bytes();

    let ciphertext = cipher
        .encrypt(&Nonce::from(nonce_bytes), secret.as_slice())
        .map_err(|_| anyhow!("encryption failed"))?;

    let mut out = Vec::with_capacity(HEADER_LEN + ciphertext.len());
    out.extend_from_slice(MAGIC);
    out.push(VERSION_HYBRID);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypts a keystore with `password`.
///
/// # Errors
///
/// Returns an error if the file is not a keystore, is a version this build does
/// not understand, is truncated, or fails authentication — which covers both a
/// wrong password and a tampered file. Those two are deliberately not
/// distinguished: telling an attacker which one they got wrong is free
/// information.
pub fn decrypt(bytes: &[u8], password: &str) -> Result<HybridSigningKey> {
    if bytes.len() < MIN_KEYSTORE_LEN || bytes.len() > MAX_KEYSTORE_LEN {
        bail!(
            "keystore must be {MIN_KEYSTORE_LEN}..={MAX_KEYSTORE_LEN} bytes, got {} — \
             file is truncated, oversized, or not a keystore",
            bytes.len()
        );
    }
    if &bytes[..8] != MAGIC {
        bail!("not a wallet keystore: bad magic");
    }

    // The two ed25519 versions are accepted here and rejected after decryption,
    // rather than rejected outright. Failing before the KDF would report an
    // unreadable file even when the password was wrong, which tells the user
    // nothing about which of the two problems they actually have.
    let version = bytes[8];
    if version != VERSION_ED25519_RAW
        && version != VERSION_ED25519_PKCS8
        && version != VERSION_MLDSA
        && version != VERSION_HYBRID
    {
        bail!(
            "unsupported keystore version {version}; this build reads \
             {VERSION_ED25519_RAW}, {VERSION_ED25519_PKCS8}, {VERSION_MLDSA} and \
             {VERSION_HYBRID}"
        );
    }

    let salt = &bytes[9..9 + SALT_LEN];
    let mut nonce_bytes = [0u8; NONCE_LEN];
    nonce_bytes.copy_from_slice(&bytes[9 + SALT_LEN..HEADER_LEN]);
    let ciphertext = &bytes[HEADER_LEN..];

    let key = derive_key(password, salt)?;
    let cipher = build_cipher(&key[..])?;

    // Zeroizing so the decrypted key material is wiped when this scope ends,
    // whichever branch below consumes it.
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(&Nonce::from(nonce_bytes), ciphertext)
            .map_err(|_| anyhow!("decryption failed: wrong password or corrupted keystore"))?,
    );

    match version {
        VERSION_HYBRID => {
            let secret = Zeroizing::new(
                <[u8; HYBRID_SECRET_KEY_LEN]>::try_from(plaintext.as_slice())
                    .context("decrypted key has the wrong length for a hybrid keystore")?,
            );
            HybridSigningKey::from_bytes(&secret)
                .map_err(|e| anyhow!("decrypted payload is not a valid hybrid key pair: {e}"))
        }
        // The password was correct and the file decrypted cleanly — these hold
        // keys from earlier eras of the chain. Say exactly that. Reporting it
        // as a corrupt file or a wrong password would send the user hunting for
        // a problem they do not have.
        VERSION_ED25519_RAW | VERSION_ED25519_PKCS8 => bail!(
            "keystore version {version} holds an ed25519 key, which this chain no longer \
             accepts. Create a new wallet with `l1-wallet new`."
        ),
        VERSION_MLDSA => bail!(
            "keystore version {version} holds an ML-DSA-65 key alone, from before the chain \
             required a second, hash-based signature. It cannot be upgraded in place: an \
             address is the hash of both public keys, so pairing this key with a newly \
             generated SLH-DSA key would name a different account holding no funds. Create \
             a new wallet with `l1-wallet new`."
        ),
        // Unreachable: the version was validated above.
        other => bail!("unsupported keystore version {other}"),
    }
}

/// Writes a keystore to `path`, refusing to clobber an existing file.
///
/// # Errors
///
/// Returns an error if the file already exists or cannot be written. Silently
/// overwriting would destroy a key with no way to recover it.
pub fn save(path: &std::path::Path, contents: &[u8]) -> Result<()> {
    if path.exists() {
        bail!(
            "{} already exists — refusing to overwrite an existing key",
            path.display()
        );
    }

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    std::fs::write(path, contents).with_context(|| format!("writing {}", path.display()))?;

    restrict_permissions(path)?;
    Ok(())
}

/// Restricts the keystore to the owner where the platform supports it.
#[cfg(unix)]
fn restrict_permissions(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(0o600);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

/// Windows has no chmod equivalent that maps cleanly here.
///
/// The file inherits the parent directory's ACL. Proper hardening would rewrite
/// the DACL to grant the owning user alone, which needs the Windows security
/// APIs; that is not done, and the caller is warned at generation time rather
/// than being left to assume the file is locked down.
#[cfg(not(unix))]
fn restrict_permissions(_path: &std::path::Path) -> Result<()> {
    Ok(())
}

/// Whether file permissions were restricted on this platform.
#[must_use]
pub const fn permissions_are_restricted() -> bool {
    cfg!(unix)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn test_key() -> HybridSigningKey {
        custom_l1_node::crypto::hybrid::generate_signing_key().expect("keygen")
    }

    #[test]
    fn encrypt_then_decrypt_recovers_the_same_key() {
        let key = test_key();
        let blob = encrypt(&key, "correct horse battery staple").expect("encrypt");
        let recovered = decrypt(&blob, "correct horse battery staple").expect("decrypt");

        assert_eq!(recovered.to_bytes(), key.to_bytes());
        assert_eq!(recovered.public_key(), key.public_key());
    }

    /// Builds a version 1 keystore: the ed25519-era layout, a bare 32-byte
    /// seed under the same envelope.
    ///
    /// Built by hand because no ed25519 key can be produced any more. What is
    /// under test is the envelope and the version handling, and both are
    /// independent of what the plaintext means.
    fn encrypt_ed25519_v1(seed: &[u8; 32], password: &str) -> Vec<u8> {
        let mut salt = [0u8; SALT_LEN];
        getrandom::fill(&mut salt).expect("entropy");
        let mut nonce_bytes = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce_bytes).expect("entropy");

        let key = derive_key(password, &salt).expect("derive");
        let cipher = build_cipher(&key[..]).expect("cipher");
        let ciphertext = cipher
            .encrypt(&Nonce::from(nonce_bytes), &seed[..])
            .expect("encrypt");

        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.push(VERSION_ED25519_RAW);
        out.extend_from_slice(&salt);
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ciphertext);
        out
    }

    #[test]
    fn keystore_has_the_documented_layout() {
        let blob = encrypt(&test_key(), "pw").expect("encrypt");
        assert_eq!(&blob[..8], MAGIC);
        assert_eq!(
            blob[8], VERSION_HYBRID,
            "writes must use the current version"
        );
        assert!(blob.len() > HEADER_LEN + TAG_LEN);
        assert!(blob.len() <= MAX_KEYSTORE_LEN);
    }

    #[test]
    fn an_ed25519_keystore_says_why_it_cannot_be_used() {
        // The correct password on a legacy file must not read as a wrong
        // password or a corrupt file. Those send the user hunting for a problem
        // they do not have; this one tells them to make a new wallet.
        let legacy = encrypt_ed25519_v1(&[4u8; 32], "legacy password");
        assert_eq!(legacy[8], VERSION_ED25519_RAW);

        let error = decrypt(&legacy, "legacy password")
            .expect_err("an ed25519 key must not be usable")
            .to_string();
        assert!(error.contains("ed25519"), "unhelpful error: {error}");
        assert!(error.contains("l1-wallet new"), "unhelpful error: {error}");
    }

    #[test]
    fn a_lattice_only_keystore_says_why_it_cannot_be_upgraded() {
        // The sharp case. A v3 file holds a perfectly valid ML-DSA key, so the
        // obvious reading is that it just needs a second key generated beside
        // it. It does not: the address commits to both keys, so that would name
        // a different account. The error has to say so, or a user will try.
        let mut salt = [0u8; SALT_LEN];
        getrandom::fill(&mut salt).expect("entropy");
        let mut nonce_bytes = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce_bytes).expect("entropy");

        let derived = derive_key("pw", &salt).expect("kdf");
        let cipher = build_cipher(&derived[..]).expect("cipher");

        // Only the lattice half, which is exactly what a v3 file held.
        let lattice_only = test_key().to_bytes()[..4032].to_vec();
        let ciphertext = cipher
            .encrypt(&Nonce::from(nonce_bytes), lattice_only.as_slice())
            .expect("encrypt");

        let mut legacy = Vec::new();
        legacy.extend_from_slice(MAGIC);
        legacy.push(VERSION_MLDSA);
        legacy.extend_from_slice(&salt);
        legacy.extend_from_slice(&nonce_bytes);
        legacy.extend_from_slice(&ciphertext);

        let error = decrypt(&legacy, "pw")
            .expect_err("a lattice-only key must not be usable")
            .to_string();
        assert!(error.contains("ML-DSA-65"), "unhelpful error: {error}");
        assert!(
            error.contains("both public keys"),
            "the error must explain why it cannot be upgraded: {error}"
        );
    }

    #[test]
    fn an_ed25519_keystore_still_reports_a_wrong_password_as_one() {
        // Version handling must not short-circuit the envelope: a wrong password
        // fails at decryption, before anything looks at what the plaintext was
        // supposed to be.
        let legacy = encrypt_ed25519_v1(&[4u8; 32], "right");
        let error = decrypt(&legacy, "wrong")
            .expect_err("wrong password must fail")
            .to_string();
        assert!(error.contains("wrong password"), "unhelpful error: {error}");
    }

    #[test]
    fn the_decrypted_payload_is_the_bare_private_key() {
        let key = test_key();
        let blob = encrypt(&key, "pw").expect("encrypt");

        // Reach past the envelope and confirm the plaintext really is the key,
        // not merely something our own decrypt happens to accept.
        let salt = &blob[9..9 + SALT_LEN];
        let mut nonce_bytes = [0u8; NONCE_LEN];
        nonce_bytes.copy_from_slice(&blob[9 + SALT_LEN..HEADER_LEN]);
        let derived = derive_key("pw", salt).expect("derive");
        let cipher = build_cipher(&derived[..]).expect("cipher");
        let plaintext = cipher
            .decrypt(&Nonce::from(nonce_bytes), &blob[HEADER_LEN..])
            .expect("decrypt");

        assert_eq!(plaintext.len(), HYBRID_SECRET_KEY_LEN);
        assert_eq!(plaintext.as_slice(), key.to_bytes().as_slice());
    }

    #[test]
    fn an_unknown_version_is_rejected() {
        let mut blob = encrypt(&test_key(), "pw").expect("encrypt");
        blob[8] = 99;
        assert!(decrypt(&blob, "pw").is_err());
    }

    #[test]
    fn an_oversized_file_is_refused_before_the_kdf_runs() {
        // Refusing early avoids a 64 MiB Argon2 pass over attacker-chosen input.
        let mut blob = encrypt(&test_key(), "pw").expect("encrypt");
        blob.resize(MAX_KEYSTORE_LEN + 1, 0);
        assert!(decrypt(&blob, "pw").is_err());
    }

    #[test]
    fn wrong_password_is_rejected() {
        let blob = encrypt(&test_key(), "right").expect("encrypt");
        assert!(decrypt(&blob, "wrong").is_err());
    }

    #[test]
    fn tampered_ciphertext_is_rejected() {
        let mut blob = encrypt(&test_key(), "pw").expect("encrypt");
        // Flip a bit in the ciphertext; Poly1305 must catch it.
        let last = blob.len() - 20;
        blob[last] ^= 0x01;
        assert!(decrypt(&blob, "pw").is_err());
    }

    #[test]
    fn tampered_salt_is_rejected() {
        let mut blob = encrypt(&test_key(), "pw").expect("encrypt");
        // A changed salt derives a different key, so authentication fails.
        blob[10] ^= 0xFF;
        assert!(decrypt(&blob, "pw").is_err());
    }

    #[test]
    fn each_encryption_uses_a_fresh_salt_and_nonce() {
        let key = test_key();
        let first = encrypt(&key, "pw").expect("encrypt");
        let second = encrypt(&key, "pw").expect("encrypt");

        // Same key and password must still produce different files: a reused
        // nonce under ChaCha20 would leak the keystream.
        assert_ne!(first, second);
        assert_ne!(first[9..9 + SALT_LEN], second[9..9 + SALT_LEN]);
        assert_ne!(
            first[9 + SALT_LEN..HEADER_LEN],
            second[9 + SALT_LEN..HEADER_LEN]
        );
    }

    #[test]
    fn bad_magic_and_version_are_rejected() {
        let mut blob = encrypt(&test_key(), "pw").expect("encrypt");
        let mut wrong_magic = blob.clone();
        wrong_magic[0] = b'X';
        assert!(decrypt(&wrong_magic, "pw").is_err());

        blob[8] = 99;
        assert!(decrypt(&blob, "pw").is_err());
    }

    #[test]
    fn truncated_keystore_is_rejected() {
        let blob = encrypt(&test_key(), "pw").expect("encrypt");
        assert!(decrypt(&blob[..blob.len() - 1], "pw").is_err());
        assert!(decrypt(&[], "pw").is_err());
    }

    #[test]
    fn empty_password_is_refused() {
        assert!(encrypt(&test_key(), "").is_err());
    }
}
