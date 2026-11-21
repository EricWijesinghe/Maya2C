//! The "update available" notice for the node (ADR-075, notify only).
//!
//! [`newer_than`] is pure: it compares versions in a manifest already in hand.
//! [`fetch_manifest`] is the one I/O step, bounded by a timeout and a size cap.
//! Nothing here downloads or installs anything.

use std::time::Duration;

/// The site's manifest for the node (ADR-075, decision 2).
pub const MANIFEST_URL: &str = "https://maya2c.dev/updates/maya2c-node.json";
/// The longest a startup check may take before it gives up.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(5);
/// The largest manifest read. The real one is a few hundred bytes.
const MAX_MANIFEST_BYTES: usize = 64 * 1024;

/// Why a manifest did not give a decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoticeError {
    /// The manifest is not JSON, or has no string `version` field.
    Malformed(String),
    /// A version is not dotted numbers such as `0.1.123`.
    BadVersion(String),
    /// The manifest could not be fetched (network, status, timeout, size).
    Fetch(String),
}

impl std::fmt::Display for NoticeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(reason) => write!(f, "update manifest is malformed: {reason}"),
            Self::BadVersion(text) => write!(f, "not a dotted version: {text:?}"),
            Self::Fetch(reason) => write!(f, "update manifest not fetched: {reason}"),
        }
    }
}

/// Fetches the manifest text. Only called when the operator passed
/// `--check-updates`; the tests never reach it.
///
/// # Errors
///
/// [`NoticeError::Fetch`] on any transport, status, timeout or size failure.
// shortcut: the body is read whole before the size check, so a hostile host can
// still make us buffer it; stream with a cap if the manifest host ever changes.
pub async fn fetch_manifest(url: &str, timeout: Duration) -> Result<String, NoticeError> {
    let fetch_error = |e: reqwest::Error| NoticeError::Fetch(e.to_string());
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(fetch_error)?;
    let response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(fetch_error)?;
    let bytes = response.bytes().await.map_err(fetch_error)?;
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(NoticeError::Fetch(format!(
            "larger than {MAX_MANIFEST_BYTES} bytes"
        )));
    }
    String::from_utf8(bytes.to_vec()).map_err(|e| NoticeError::Malformed(e.to_string()))
}

/// Fetches the manifest at `url` and returns the newer version, if any.
///
/// # Errors
///
/// Any [`NoticeError`] from [`fetch_manifest`] or [`newer_than`].
pub async fn check(current: &str, url: &str) -> Result<Option<String>, NoticeError> {
    let manifest = fetch_manifest(url, FETCH_TIMEOUT).await?;
    newer_than(current, &manifest)
}

/// Dotted numbers as a vector, so `0.1.10` compares above `0.1.9`.
fn parse_version(text: &str) -> Option<Vec<u64>> {
    text.split('.')
        .map(|part| part.parse::<u64>().ok())
        .collect()
}

/// The version the manifest publishes when it is newer than `current`.
///
/// `Ok(None)` means the node is up to date (or ahead of the manifest).
///
/// # Errors
///
/// [`NoticeError::Malformed`] if the manifest is not JSON or has no string
/// `version`; [`NoticeError::BadVersion`] if either version is not dotted
/// numbers.
pub fn newer_than(current: &str, manifest: &str) -> Result<Option<String>, NoticeError> {
    let value: serde_json::Value =
        serde_json::from_str(manifest).map_err(|e| NoticeError::Malformed(e.to_string()))?;
    let published = value
        .get("version")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| NoticeError::Malformed("no string `version` field".to_owned()))?;
    let installed =
        parse_version(current).ok_or_else(|| NoticeError::BadVersion(current.to_owned()))?;
    let latest =
        parse_version(published).ok_or_else(|| NoticeError::BadVersion(published.to_owned()))?;
    Ok((latest > installed).then(|| published.to_owned()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn manifest(version: &str) -> String {
        format!(r#"{{"version":"{version}","notes":"n","pub_date":"2026-11-01"}}"#)
    }

    #[test]
    fn a_newer_published_version_is_reported() {
        assert_eq!(
            newer_than("0.1.123", &manifest("0.1.124")).unwrap(),
            Some("0.1.124".to_owned())
        );
    }

    #[test]
    fn the_same_or_an_older_published_version_is_not() {
        assert_eq!(newer_than("0.1.124", &manifest("0.1.124")).unwrap(), None);
        assert_eq!(newer_than("0.1.125", &manifest("0.1.124")).unwrap(), None);
    }

    #[test]
    fn versions_compare_as_numbers_not_text() {
        // As text "0.1.10" sorts below "0.1.9"; as numbers it is newer.
        assert_eq!(
            newer_than("0.1.9", &manifest("0.1.10")).unwrap(),
            Some("0.1.10".to_owned())
        );
    }

    #[test]
    fn a_manifest_without_a_version_is_an_error_not_a_quiet_no() {
        assert!(matches!(
            newer_than("0.1.1", r#"{"notes":"n"}"#),
            Err(NoticeError::Malformed(_))
        ));
        assert!(matches!(
            newer_than("0.1.1", "not json"),
            Err(NoticeError::Malformed(_))
        ));
    }

    #[test]
    fn a_non_numeric_version_is_an_error() {
        assert!(matches!(
            newer_than("0.1.1", &manifest("0.1.x")),
            Err(NoticeError::BadVersion(_))
        ));
        assert!(matches!(
            newer_than("dev", &manifest("0.1.2")),
            Err(NoticeError::BadVersion(_))
        ));
    }
}
