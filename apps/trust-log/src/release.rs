//! `release-log-anchor`: a signed statement of a release's `SHA256SUMS`
//! digest, anchored on chain under `maya2c/release/<tag>`.
//!
//! The release host (GitHub, Rekor) can swap `SHA256SUMS` and every file it
//! lists; it cannot produce a statement under the pinned key, and it cannot
//! move the anchor's height back. A mismatch is reported, never acted on —
//! nothing here installs, deletes or refuses to run anything.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::anchor::Anchor;
use crate::key::SignerKey;
use crate::signed::{self, Signed};
use crate::{Error, Result};

/// Digest domain of a release statement.
pub const DOMAIN: &str = "maya2c 2026-11-12 release-log-anchor v1";
/// Schema prefix; the tag follows.
pub const SCHEMA_PREFIX: &str = "maya2c/release/";

/// What the foundation states about one release.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseStatement {
    /// The release tag, e.g. `v1.0.0`.
    pub tag: String,
    /// Hex SHA-256 of the release's `SHA256SUMS` file, byte for byte.
    pub sha256sums_sha256: String,
}

/// The anchor schema for `tag`.
///
/// # Errors
///
/// [`Error::Malformed`] if the schema would pass the node's bound.
pub fn anchor_for(tag: &str, digest: [u8; 32]) -> Result<Anchor> {
    Anchor::new(format!("{SCHEMA_PREFIX}{tag}"), digest)
}

/// Signs the statement for `tag` over the bytes of its `SHA256SUMS`.
///
/// # Errors
///
/// As [`signed::seal`] and [`anchor_for`].
pub fn sign(key: &SignerKey, tag: &str, sums: &[u8]) -> Result<(Signed<ReleaseStatement>, Anchor)> {
    let body = ReleaseStatement {
        tag: tag.to_owned(),
        sha256sums_sha256: hex::encode(Sha256::digest(sums)),
    };
    let statement = signed::seal(key, DOMAIN, body)?;
    let anchor = anchor_for(tag, crate::hex_array("digest", &statement.digest)?)?;
    Ok((statement, anchor))
}

/// Checks a fetched `SHA256SUMS` against the statement for `expected_tag`,
/// and returns the anchor the chain must show.
///
/// `expected_tag` is the tag being installed, not the statement's: a genuine
/// statement for an older release must not vouch for a newer download.
///
/// # Errors
///
/// [`Error::Signature`] for a statement the pinned key did not sign, and
/// [`Error::Release`] for another tag or a `SHA256SUMS` that differs.
pub fn verify(
    statement: &Signed<ReleaseStatement>,
    pinned: &[u8],
    expected_tag: &str,
    sums: &[u8],
) -> Result<Anchor> {
    let digest = statement.open(DOMAIN, pinned)?;
    let body = &statement.body;
    if body.tag != expected_tag {
        return Err(Error::Release(format!(
            "statement is for {}, not {expected_tag}",
            body.tag
        )));
    }
    let fetched = hex::encode(Sha256::digest(sums));
    if body.sha256sums_sha256 != fetched {
        return Err(Error::Release(format!(
            "SHA256SUMS hashes to {fetched}, the signed statement says {}",
            body.sha256sums_sha256
        )));
    }
    anchor_for(&body.tag, digest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signed::tests::key;

    const SUMS: &[u8] = b"abc  maya2c-node-linux\n";

    #[test]
    fn the_signed_release_verifies_and_names_its_anchor() {
        let k = key(1);
        let (s, a) = sign(&k, "v1.0.0", SUMS).unwrap();
        assert_eq!(verify(&s, k.public_key(), "v1.0.0", SUMS).unwrap(), a);
        assert_eq!(a.schema, "maya2c/release/v1.0.0");
    }

    #[test]
    fn a_swapped_sha256sums_is_reported() {
        let k = key(1);
        let (s, _) = sign(&k, "v1.0.0", SUMS).unwrap();
        let swapped = b"abd  maya2c-node-linux\n";
        assert!(matches!(
            verify(&s, k.public_key(), "v1.0.0", swapped),
            Err(Error::Release(_))
        ));
    }

    #[test]
    fn an_old_statement_does_not_vouch_for_a_new_tag() {
        let k = key(1);
        let (s, _) = sign(&k, "v1.0.0", SUMS).unwrap();
        assert!(matches!(
            verify(&s, k.public_key(), "v1.0.1", SUMS),
            Err(Error::Release(_))
        ));
    }

    #[test]
    fn a_statement_from_another_key_is_refused() {
        let (k, attacker) = (key(1), key(9));
        let (s, _) = sign(&attacker, "v1.0.0", SUMS).unwrap();
        assert!(matches!(
            verify(&s, k.public_key(), "v1.0.0", SUMS),
            Err(Error::Signature(_))
        ));
    }
}
