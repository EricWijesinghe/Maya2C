//! Errors from sealing, loading and binding.

use maya_ebpf_net_common::HeaderError;

/// Why a relay operation failed.
#[derive(Debug, thiserror::Error)]
pub enum RelayError {
    /// A header that cannot exist.
    #[error("relay header: {0}")]
    Header(#[from] HeaderError),
    /// A chunk's plaintext is not the length its header implies.
    #[error("chunk plaintext is {actual} bytes; its header implies {expected}")]
    PlaintextLength {
        /// Bytes supplied.
        actual: usize,
        /// Bytes the header implies.
        expected: usize,
    },
    /// A header names a different key than the one asked to seal or open it.
    #[error("header names relay key {named:#018x}, not {held:#018x}")]
    WrongKey {
        /// The key id in the header.
        named: u64,
        /// The key id actually held.
        held: u64,
    },
    /// A chunk failed authentication.
    #[error("chunk failed authentication")]
    Forged,
    /// A block body too large for any header to describe.
    #[error("a {0}-byte block exceeds the relay ceiling")]
    BodyTooLarge(usize),
    /// The kernel path is not available in this build or on this platform.
    #[error("the XDP receive path is unavailable: {0}")]
    Unsupported(&'static str),
    /// A configuration the kernel path cannot use.
    #[error("invalid XDP configuration: {0}")]
    Config(String),
    /// A system call failed.
    #[error("{context}: {source}")]
    Io {
        /// What was being attempted.
        context: &'static str,
        /// The OS error.
        #[source]
        source: std::io::Error,
    },
    /// Loading, attaching, or a map operation on the XDP program failed.
    #[error("eBPF {context}: {message}")]
    Ebpf {
        /// What was being attempted.
        context: &'static str,
        /// aya's description.
        message: String,
    },
}

impl RelayError {
    /// Wraps the current OS error with `context`.
    #[must_use]
    pub fn last_os_error(context: &'static str) -> Self {
        Self::Io {
            context,
            source: std::io::Error::last_os_error(),
        }
    }
}
