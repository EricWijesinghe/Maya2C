//! The "update available" decision for the node (ADR-075, notify only).
//!
//! Pure: it reads a manifest that has already been fetched and compares
//! versions. Nothing here downloads or installs anything, and the fetch itself
//! is not written yet (ADR-075, "Shipped in this change").

// shortcut: no caller until the fetch step lands, then drop this allow (ADR-075).
#![allow(dead_code)]

/// Why a manifest did not give a decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoticeError {
    /// The manifest is not JSON, or has no string `version` field.
    Malformed(String),
    /// A version is not dotted numbers such as `0.1.123`.
    BadVersion(String),
}

impl std::fmt::Display for NoticeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(reason) => write!(f, "update manifest is malformed: {reason}"),
            Self::BadVersion(text) => write!(f, "not a dotted version: {text:?}"),
        }
    }
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
