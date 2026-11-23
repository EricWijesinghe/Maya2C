//! The checks an update must pass before it is installed, kept free of Tauri
//! so they can be tested without a window.
//!
//! The keys and signatures are the ones `cargo tauri signer` writes: base64 of
//! a minisign text file. The Tauri updater plugin checks the signature first:
//! `Update::download` and `Update::install` both call its `verify_signature`
//! (tauri-plugin-updater 2.14.1, `updater.rs`; 2.13.3's `install` did not, so
//! `Cargo.toml` requires 2.14.1 and all three apps lock it), so an unsigned or wrong-key
//! bundle normally fails there, with the plugin's error text. [`verify`] is a
//! second, stricter check behind it: it refuses legacy (non-prehashed)
//! signatures, which the plugin accepts, and a signature whose signed
//! `version:` is missing or differs from the announced version, which the
//! plugin accepts unless `requireSignedVersion` is set. The manifest itself is
//! not signed, so that binding is what stops a manifest host from replaying an
//! older signed bundle under a newer version number. The tests below pin both
//! whatever a plugin upgrade does to the first check.

use std::error::Error;
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
    /// The signature does not bind the version the manifest announced.
    VersionMismatch,
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unsigned => "the update is not signed; refused",
            Self::BadKey => "this build's update key cannot be read",
            Self::BadSignature => "the update signature cannot be read; refused",
            Self::Refused => "the update is not signed by this app's publisher key; refused",
            Self::VersionMismatch => {
                "the update signature is not for the version announced; refused"
            }
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

/// SHA-256 of `bytes` as `sha256sum` prints it (64 lower-case hex digits), so
/// it can be compared character for character with a release's `SHA256SUMS`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// SHA-256 of the raw public key (algorithm, key id and Ed25519 key), in
/// [`sha256_hex`]'s format. The key id alone is not a fingerprint: it is
/// chosen at random when the key is made, so another key can carry it.
///
/// Anyone can reproduce it from `apps/updater-pubkey.txt`, in the same format:
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
    Ok(sha256_hex(&raw))
}

/// The install path after the download, in the only order it may run: the
/// manifest's `signature` must verify over `bytes` for `version`, then the user must accept
/// `confirm` (shown the download's [`sha256_hex`]), and only then `install`.
/// Returns whether it installed.
///
/// # Errors
///
/// The [`VerifyError`] of a refused signature (neither `confirm` nor
/// `install` runs), or `install`'s own error.
pub fn verify_confirm_install(
    pubkey: &str,
    signature: &str,
    version: &str,
    bytes: Vec<u8>,
    confirm: impl FnOnce(&str) -> bool,
    install: impl FnOnce(Vec<u8>) -> Result<(), Box<dyn Error + Send + Sync>>,
) -> Result<bool, Box<dyn Error + Send + Sync>> {
    verify(pubkey, signature, version, &bytes)?;
    if !confirm(&sha256_hex(&bytes)) {
        return Ok(false);
    }
    install(bytes)?;
    Ok(true)
}

/// Checks that `data` was signed by `pubkey` as `version`. An empty
/// signature, one from another key, changed bytes, and a signature whose
/// trusted comment does not carry `version:<version>` are all refused.
///
/// # Errors
///
/// The [`VerifyError`] that says which check failed.
pub fn verify(
    pubkey: &str,
    signature: &str,
    version: &str,
    data: &[u8],
) -> Result<(), VerifyError> {
    if signature.trim().is_empty() {
        return Err(VerifyError::Unsigned);
    }
    let key = public_key(pubkey)?;
    let text = unwrap_b64(signature).ok_or(VerifyError::BadSignature)?;
    let signature = Signature::decode(&text).map_err(|_| VerifyError::BadSignature)?;
    // `false`: no legacy (non-prehashed) signatures; `cargo tauri signer`
    // never writes one, so accepting them would only widen what passes.
    key.verify(data, &signature, false)
        .map_err(|_| VerifyError::Refused)?;
    // The trusted comment is covered by the signature checked above. The
    // tauri CLI writes it as tab-separated `key:value` fields.
    let signed = signature
        .trusted_comment()
        .split('\t')
        .find_map(|field| field.strip_prefix("version:"));
    match signed {
        Some(signed) if signed.trim_start_matches('v') == version.trim_start_matches('v') => {
            Ok(())
        }
        _ => Err(VerifyError::VersionMismatch),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Throwaway keys made for these tests with `cargo tauri signer generate`
    // (empty password); the secret halves were deleted. SIG_A is
    // `cargo tauri signer sign` by key A over PAYLOAD with no version (its
    // trusted comment has only timestamp and file). SIG_C is tauri CLI 2.12.2
    // `signer sign --app-version 0.1.7` by key C over PAYLOAD, the way the
    // updater-sign release job signs.
    const KEY_A: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDdFODRFRUVDMkZERTEzOUYKUldTZkU5NHY3TzZFZm51TXk3WmRFY1MxQjBhcWU3NUxiMU41b3gySmVpZ0p0YVFmZGlhTVlDZ2kK";
    const KEY_B: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDhGOTM5M0NGRjgyQUVCMUMKUldRYzZ5cjR6NU9UaitWeUVxa3FBMzJLQ3FSSTFTVWF5dVRwRnF6TWJISTZCSHRJZ3B3NTRyb3UK";
    const SIG_A: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVTZkU5NHY3TzZFZmcrUTkzdHVNbFFDN0xMSVZUdmFmNU9GNU9qOXp6c2tPZXVXVmhGbHdud01TQ2xGOEU2dUZpVVBOTXdxdUxPZ2RoaGNEek1KVjJ2STNPazR1Zm4zUndvPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk0NTg3NzYzCWZpbGU6cGF5bG9hZC5iaW4KaGNVVEloenRvWjBVWTE5eTFkVWZtREJWckdMS05ZSmo4aXVHWmtDNzhDSlJXOTZpZWw0ZWtLZ3NCZWRZbjRjYUhyVjdDZXM3ekVKSnJjNFRDOUxmQkE9PQo=";
    const KEY_C: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEYyMTc0REUzMTU0NjNDRjcKUldUM1BFWVY0MDBYOG1sSTRBSnFkaThBL2YyeGliK3lxNU9kak1IMFRJSHdMNlB1OUtJWTFKZWsK";
    const SIG_C: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVUM1BFWVY0MDBYOGtPNStxdmRjVlc5NVRjZ3I0RWVHRk5xT1M4SXYzMzdVd1ZmenRQLzduOGtmb2E5bkMxTDBLVlVOb3k0L25sV0JZWFAwK2k3eHNVZFNYVDA1ZGtYbFFzPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk1MjU3MDY4CWZpbGU6cGF5bG9hZC5iaW4JdmVyc2lvbjowLjEuNwo0c2NHdEpEU1UyK2hsMCtjbDBWMTBoK1VWVUltazBwMDFFNlRyaWVpTmZjc1V1U0xVUitrRW41ckFuM3FHYzNJMXUvV1hoNlFHM3VqSVNIeDBTc0hEZz09Cg==";
    // KEY_D is a raw Ed25519 key built in minisign format with Python
    // `cryptography` (secret discarded), because `cargo tauri signer` cannot
    // write a legacy signature. Both signatures are over PAYLOAD with trusted
    // comment `timestamp:1795257068\tfile:payload.bin\tversion:0.1.7`:
    // SIG_D_PREHASHED is algorithm `ED` (Ed25519 over BLAKE2b-512 of PAYLOAD,
    // what the tauri CLI writes), SIG_D_LEGACY is algorithm `Ed` (Ed25519
    // over PAYLOAD itself).
    const KEY_D: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDRBMTM5MjgzMTlDNDgxOEEKUldTS2djUVpnNUlUU296a3ZkU2FsTTRHbTlJRlpaWkg2TFI1aXUwNkZNRUkrOWowb3B2MmlzSmQK";
    const SIG_D_PREHASHED: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVTS2djUVpnNUlUU28zaitYSy8rZlhYYStUTmV5ZnV3Umk2YjZDRG9CUnZZaXdFRHRKRWV0U1k5Yis3TVpkNFU5SVA1aVlRb0dkRTg0eDA0QU02Sk5IYlJhR0VJUEdpZXdFPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk1MjU3MDY4CWZpbGU6cGF5bG9hZC5iaW4JdmVyc2lvbjowLjEuNwpueTFISWV5cW5TWGVvajRMQ010b1BtaC9GVFQrTzZhY01kYndvMnFzRHhwbFg1SjU1S2tHeS9YUVZmZTNaOXRFRnhZNmNyZmp5UWVwa1NBNHhMRHFCZz09Cg==";
    const SIG_D_LEGACY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUldTS2djUVpnNUlUU3NTem9sWGFUS2tNUHBoZkVCOFluc0gwYnNWMG8rRGJOQWtML1h3ZkJGMTBrL0VtQWNwRjJ0VVp2WUNQY1pjYUJFQ2FtQkpWNWo4WEFSanVEUm1ZYkFzPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk1MjU3MDY4CWZpbGU6cGF5bG9hZC5iaW4JdmVyc2lvbjowLjEuNwoyYUxSMjdadkY1ZFRYZlRYL0dEU21pbnJvUlQrdWJGSlA2UFltYU5mSmJ2Y1Jmbk9DcTNSSHhqWE1vQWRhWmxVN1czWWJxZjBZSmNuc2NzUUk4UXNDUT09Cg==";
    const SIGNED: &str = "0.1.7";
    const PAYLOAD: &[u8] = b"maya2c test update payload\n";

    /// The one property that makes [`verify`] stricter than the plugin's own
    /// check: a legacy (non-prehashed) signature by the publisher key, over the
    /// right bytes and version, is still refused. The prehashed twin passes,
    /// so the refusal is the algorithm and not a broken vector.
    #[test]
    fn legacy_signature_by_the_publisher_key_is_refused() {
        assert_eq!(verify(KEY_D, SIG_D_PREHASHED, SIGNED, PAYLOAD), Ok(()));
        assert_eq!(
            verify(KEY_D, SIG_D_LEGACY, SIGNED, PAYLOAD),
            Err(VerifyError::Refused)
        );
    }

    #[test]
    fn signed_by_the_publisher_key_passes() {
        assert_eq!(verify(KEY_C, SIG_C, SIGNED, PAYLOAD), Ok(()));
        assert_eq!(verify(KEY_C, SIG_C, "v0.1.7", PAYLOAD), Ok(()));
    }

    #[test]
    fn unsigned_bundle_is_refused() {
        assert_eq!(verify(KEY_C, "", SIGNED, PAYLOAD), Err(VerifyError::Unsigned));
        assert_eq!(
            verify(KEY_C, " \n", SIGNED, PAYLOAD),
            Err(VerifyError::Unsigned)
        );
    }

    #[test]
    fn signature_from_another_key_is_refused() {
        assert_eq!(
            verify(KEY_B, SIG_C, SIGNED, PAYLOAD),
            Err(VerifyError::Refused)
        );
    }

    #[test]
    fn changed_download_is_refused() {
        assert_eq!(
            verify(KEY_C, SIG_C, SIGNED, b"maya2c test update payload!\n"),
            Err(VerifyError::Refused)
        );
    }

    /// The rollback replay: a genuine signature served under an announced
    /// version other than the one it signed, or one that signed no version.
    #[test]
    fn signature_for_another_version_or_none_is_refused() {
        for announced in ["0.1.8", "0.1.70", "0.1"] {
            assert_eq!(
                verify(KEY_C, SIG_C, announced, PAYLOAD),
                Err(VerifyError::VersionMismatch),
                "{announced}"
            );
        }
        // SIG_A is genuine for key A but binds no version.
        assert_eq!(
            verify(KEY_A, SIG_A, SIGNED, PAYLOAD),
            Err(VerifyError::VersionMismatch)
        );
    }

    #[test]
    fn garbage_signature_and_key_are_refused() {
        assert_eq!(
            verify(KEY_C, "not base64 !!", SIGNED, PAYLOAD),
            Err(VerifyError::BadSignature)
        );
        assert_eq!(
            verify(KEY_C, KEY_C, SIGNED, PAYLOAD),
            Err(VerifyError::BadSignature)
        );
        assert_eq!(verify("", SIG_C, SIGNED, PAYLOAD), Err(VerifyError::BadKey));
        assert_eq!(fingerprint("bm90IGEga2V5"), Err(VerifyError::BadKey));
    }

    #[test]
    fn fingerprint_is_sha256_of_the_raw_key() {
        // Expected values from `base64 -d | sed -n 2p | base64 -d | sha256sum`,
        // in that command's own format so a user can compare them as printed.
        assert_eq!(
            fingerprint(KEY_A).as_deref(),
            Ok("7ee692aac59f0c2133ff01e6ed533019af554f84d7721401a7cda0aa454e0623")
        );
        assert_eq!(
            fingerprint(KEY_B).as_deref(),
            Ok("96712cdeabab47df37f2cfc0260e57a220f8915c07e9803cba23ca647923bf0f")
        );
    }

    /// Runs the gate and records what it reached: the digest it asked about,
    /// and the bytes it installed.
    fn gate(
        pubkey: &str,
        signature: &str,
        version: &str,
        accept: bool,
    ) -> (Result<bool, VerifyError>, Option<String>, Option<Vec<u8>>) {
        let mut asked = None;
        let mut installed = None;
        let result = verify_confirm_install(
            pubkey,
            signature,
            version,
            PAYLOAD.to_vec(),
            |digest| {
                asked = Some(digest.to_owned());
                accept
            },
            |bytes| {
                installed = Some(bytes);
                Ok(())
            },
        )
        .map_err(|error| {
            error
                .downcast_ref::<VerifyError>()
                .cloned()
                .expect("a refusal is a VerifyError")
        });
        (result, asked, installed)
    }

    #[test]
    fn unsigned_bundle_never_reaches_the_question_or_the_install() {
        assert_eq!(
            gate(KEY_C, "", SIGNED, true),
            (Err(VerifyError::Unsigned), None, None)
        );
    }

    #[test]
    fn wrong_key_bundle_signature_never_reaches_the_question_or_the_install() {
        assert_eq!(
            gate(KEY_B, SIG_C, SIGNED, true),
            (Err(VerifyError::Refused), None, None)
        );
    }

    #[test]
    fn wrong_version_bundle_signature_never_reaches_the_question_or_the_install() {
        assert_eq!(
            gate(KEY_C, SIG_C, "0.1.8", true),
            (Err(VerifyError::VersionMismatch), None, None)
        );
    }

    #[test]
    fn signed_download_asks_with_its_sha256sum_then_installs_those_bytes() {
        // `printf 'maya2c test update payload\n' | sha256sum`.
        let digest = "079a98ba41136036a6c3f39c354820f3ed3b96afb1b91e1c2bb447b14219687f";
        assert_eq!(
            gate(KEY_C, SIG_C, SIGNED, true),
            (Ok(true), Some(digest.to_owned()), Some(PAYLOAD.to_vec()))
        );
    }

    #[test]
    fn declined_download_is_not_installed() {
        let (result, asked, installed) = gate(KEY_C, SIG_C, SIGNED, false);
        assert_eq!((result, asked.is_some(), installed), (Ok(false), true, None));
    }
}
