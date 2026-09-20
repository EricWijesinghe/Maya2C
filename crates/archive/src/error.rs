//! Archive errors.

/// Everything that can go wrong building, storing, fetching or opening an
/// archive.
///
/// Every variant that comes from untrusted bytes is a refusal, not a partial
/// result: an archive either verifies completely or is not used.
#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    /// The CAR framing is malformed: bad varint, truncated section, bad header.
    #[error("malformed CAR: {0}")]
    Car(String),
    /// A content identifier is malformed or uses a hash this crate does not
    /// verify.
    #[error("bad CID: {0}")]
    Cid(String),
    /// A section's bytes do not hash to the CID it is filed under.
    #[error("section {cid} does not hash to its CID")]
    ContentMismatch {
        /// The CID the section claimed.
        cid: String,
    },
    /// The manifest is malformed or disagrees with the sections present.
    #[error("bad manifest: {0}")]
    Manifest(String),
    /// A store returned an archive other than the one requested.
    #[error("store returned root {actual}, expected {expected}")]
    WrongRoot {
        /// Root that was asked for.
        expected: String,
        /// Root that came back.
        actual: String,
    },
    /// Decompressing or decoding would exceed the size bound.
    #[error("archive exceeds {limit} bytes")]
    TooLarge {
        /// The bound that was hit.
        limit: usize,
    },
    /// The store cannot perform this operation.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// A filesystem failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// An HTTP failure talking to a remote store.
    #[error("http: {0}")]
    Http(String),
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, ArchiveError>;
