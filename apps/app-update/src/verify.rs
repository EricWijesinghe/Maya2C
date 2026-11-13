//! The checks an update must pass before it is installed, kept free of Tauri
//! so they can be tested without a window.
//!
//! The keys and signatures are the ones `cargo tauri signer` writes: base64 of
//! a minisign text file. The Tauri updater plugin checks the same signature
//! when it downloads; [`verify`] repeats the check in this crate's install path
//! so the refusal is ours, and pinned by the tests below.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use minisign_verify::{PublicKey, Signature};
use sha2::{Digest as _, Sha256};

/// Why an update was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    /// The manifest carries no signature for the download.
    Unsigned,
    /// The compiled-in publisher key cannot be read.
    BadKey,
    /// The signature is not a minisign signature.
    BadSignature,
    /// The signature was made by a different key, or the bytes were changed.
    Refused,
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unsigned => "the update is not signed; refused",
            Self::BadKey => "this build's update key cannot be read",
            Self::BadSignature => "the update signature cannot be read; refused",
            Self::Refused => "the update is not signed by this app's publisher key; refused",
        })
    }
}

impl std::error::Error for VerifyError {}

/// Base64 of a minisign text file, decoded to that text.
fn unwrap_b64(text: &str) -> Option<String> {
    String::from_utf8(STANDARD.decode(text.trim()).ok()?).ok()
}

fn public_key(pubkey: &str) -> Result<PublicKey, VerifyError> {
    let text = unwrap_b64(pubkey).ok_or(VerifyError::BadKey)?;
    PublicKey::decode(&text).map_err(|_| VerifyError::BadKey)
}

/// SHA-256 of the raw public key (algorithm, key id and Ed25519 key), in
/// groups of four hex digits. The key id alone is not a fingerprint: it is
/// chosen at random when the key is made, so another key can carry it.
///
/// Anyone can reproduce it from `apps/updater-pubkey.txt`:
/// `base64 -d updater-pubkey.txt | sed -n 2p | base64 -d | sha256sum`.
///
/// # Errors
///
/// [`VerifyError::BadKey`] when `pubkey` is not a minisign public key.
pub fn fingerprint(pubkey: &str) -> Result<String, VerifyError> {
    public_key(pubkey)?;
    let text = unwrap_b64(pubkey).ok_or(VerifyError::BadKey)?;
    let line = text.lines().nth(1).ok_or(VerifyError::BadKey)?;
    let raw = STANDARD
        .decode(line.trim())
        .map_err(|_| VerifyError::BadKey)?;
    let hex: Vec<String> = Sha256::digest(raw)
        .chunks(2)
        .map(|pair| format!("{:02X}{:02X}", pair[0], pair[1]))
        .collect();
    Ok(hex.join(" "))
}

/// Checks that `data` was signed by `pubkey`. An empty signature, one from
/// another key, and changed bytes are all refused.
///
/// # Errors
///
/// The [`VerifyError`] that says which check failed.
pub fn verify(pubkey: &str, signature: &str, data: &[u8]) -> Result<(), VerifyError> {
    if signature.trim().is_empty() {
        return Err(VerifyError::Unsigned);
    }
    let key = public_key(pubkey)?;
    let text = unwrap_b64(signature).ok_or(VerifyError::BadSignature)?;
    let signature = Signature::decode(&text).map_err(|_| VerifyError::BadSignature)?;
    // `false`: no legacy (non-prehashed) signatures; `cargo tauri signer`
    // never writes one, so accepting them would only widen what passes.
    key.verify(data, &signature, false)
        .map_err(|_| VerifyError::Refused)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Throwaway keys made for these tests with `cargo tauri signer generate`
    // (empty password); the secret halves were deleted. SIG_A is
    // `cargo tauri signer sign` by key A over PAYLOAD.
    const KEY_A: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDdFODRFRUVDMkZERTEzOUYKUldTZkU5NHY3TzZFZm51TXk3WmRFY1MxQjBhcWU3NUxiMU41b3gySmVpZ0p0YVFmZGlhTVlDZ2kK";
    const KEY_B: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDhGOTM5M0NGRjgyQUVCMUMKUldRYzZ5cjR6NU9UaitWeUVxa3FBMzJLQ3FSSTFTVWF5dVRwRnF6TWJISTZCSHRJZ3B3NTRyb3UK";
    const SIG_A: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVTZkU5NHY3TzZFZmcrUTkzdHVNbFFDN0xMSVZUdmFmNU9GNU9qOXp6c2tPZXVXVmhGbHdud01TQ2xGOEU2dUZpVVBOTXdxdUxPZ2RoaGNEek1KVjJ2STNPazR1Zm4zUndvPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk0NTg3NzYzCWZpbGU6cGF5bG9hZC5iaW4KaGNVVEloenRvWjBVWTE5eTFkVWZtREJWckdMS05ZSmo4aXVHWmtDNzhDSlJXOTZpZWw0ZWtLZ3NCZWRZbjRjYUhyVjdDZXM3ekVKSnJjNFRDOUxmQkE9PQo=";
    const PAYLOAD: &[u8] = b"maya2c test update payload\n";

    #[test]
    fn signed_by_the_publisher_key_passes() {
        assert_eq!(verify(KEY_A, SIG_A, PAYLOAD), Ok(()));
    }

    #[test]
    fn unsigned_manifest_is_refused() {
        assert_eq!(verify(KEY_A, "", PAYLOAD), Err(VerifyError::Unsigned));
        assert_eq!(verify(KEY_A, " \n", PAYLOAD), Err(VerifyError::Unsigned));
    }

    #[test]
    fn signature_from_another_key_is_refused() {
        assert_eq!(verify(KEY_B, SIG_A, PAYLOAD), Err(VerifyError::Refused));
    }

    #[test]
    fn changed_download_is_refused() {
        assert_eq!(
            verify(KEY_A, SIG_A, b"maya2c test update payload!\n"),
            Err(VerifyError::Refused)
        );
    }

    #[test]
    fn garbage_signature_and_key_are_refused() {
        assert_eq!(
            verify(KEY_A, "not base64 !!", PAYLOAD),
            Err(VerifyError::BadSignature)
        );
        assert_eq!(
            verify(KEY_A, KEY_A, PAYLOAD),
            Err(VerifyError::BadSignature)
        );
        assert_eq!(verify("", SIG_A, PAYLOAD), Err(VerifyError::BadKey));
        assert_eq!(fingerprint("bm90IGEga2V5"), Err(VerifyError::BadKey));
    }

    #[test]
    fn fingerprint_is_sha256_of_the_raw_key() {
        // Expected values from `base64 -d | sed -n 2p | base64 -d | sha256sum`.
        assert_eq!(
            fingerprint(KEY_A).as_deref(),
            Ok("7EE6 92AA C59F 0C21 33FF 01E6 ED53 3019 AF55 4F84 D772 1401 A7CD A0AA 454E 0623")
        );
        assert_eq!(
            fingerprint(KEY_B).as_deref(),
            Ok("9671 2CDE ABAB 47DF 37F2 CFC0 260E 57A2 20F8 915C 07E9 803C BA23 CA64 7923 BF0F")
        );
    }
}
