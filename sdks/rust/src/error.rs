//! Why a call failed.

use std::fmt;
use std::time::Duration;

/// A non-2xx answer from the gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    /// HTTP status.
    pub status: u16,
    /// The gateway's `code` field when it sends one, else the status reason in
    /// snake case (`not_found`, `bad_request`). The gateway currently sends only
    /// `error`, so this is usually the reason.
    pub code: String,
    /// The gateway's `error` text. It is already flattened for public callers:
    /// the node's own message never reaches an SDK user.
    pub detail: String,
    /// The `Retry-After` header in seconds, if the gateway sent one.
    pub retry_after: Option<Duration>,
}

/// Every way a call can fail.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The gateway answered with an error status other than 429.
    #[error("API error {}: {}", .0.status, .0.detail)]
    Api(ApiError),
    /// The gateway kept answering 429 after the SDK's retries ran out.
    #[error("rate limited (HTTP 429): {}", .0.detail)]
    RateLimited(ApiError),
    /// The gateway could not be reached: DNS, connection refused, reset.
    #[error("network error: {0}")]
    Network(String),
    /// No answer within the client's timeout, or the wait's deadline passed.
    #[error("timed out")]
    Timeout,
    /// The base URL is not an HTTP(S) URL.
    #[error("invalid base URL: {0}")]
    Config(String),
    /// An address is neither 64 hex characters nor a valid Bech32m address.
    /// Refused before any request is sent.
    #[error("invalid address: {0}")]
    InvalidAddress(String),
    /// A successful answer did not have the expected shape.
    #[error("unexpected response: {0}")]
    Decode(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}: {}", self.status, self.code, self.detail)
    }
}

impl From<maya_address::AddressError> for Error {
    fn from(error: maya_address::AddressError) -> Self {
        Self::InvalidAddress(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn api_error_message_carries_status_and_gateway_text() {
        let error = Error::Api(ApiError {
            status: 400,
            code: "bad_request".into(),
            detail: "address must be 64 hex characters".into(),
            retry_after: None,
        });
        assert_eq!(
            error.to_string(),
            "API error 400: address must be 64 hex characters"
        );
    }

    #[test]
    fn address_errors_become_invalid_address_without_a_request() {
        let error: Error = maya_address::decode_any("maya1nope")
            .expect_err("bad checksum")
            .into();
        assert!(matches!(error, Error::InvalidAddress(_)));
    }
}
