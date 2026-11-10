//! One error type for the service.
//!
//! Messages a caller can see are written to be safe to show: none carries a
//! secret, a raw RPC body or a file path. Internal detail goes to the log.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// Everything the checkout service can refuse or fail at.
#[derive(Debug, thiserror::Error)]
pub enum CheckoutError {
    /// The caller sent something invalid. The message says what, for them.
    #[error("{0}")]
    BadRequest(String),
    /// No bearer token, or the wrong one.
    #[error("unauthorized")]
    Unauthorized,
    /// No such invoice.
    #[error("not found")]
    NotFound,
    /// The request clashes with state the service already holds.
    #[error("{0}")]
    Conflict(String),
    /// The chain node did not answer. Detail is logged, not returned.
    #[error("chain node unavailable: {0}")]
    Chain(String),
    /// The ledger file could not be read or written. Detail is logged only.
    #[error("store failure: {0}")]
    Store(String),
    /// A programming or system fault (randomness, encoding). Logged only.
    #[error("internal failure: {0}")]
    Internal(String),
    /// Startup configuration is missing or wrong.
    #[error("configuration: {0}")]
    Config(String),
}

/// Result alias used across the crate.
pub type Result<T> = std::result::Result<T, CheckoutError>;

impl IntoResponse for CheckoutError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            Self::BadRequest(m) => (StatusCode::BAD_REQUEST, m.clone()),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".to_owned()),
            Self::NotFound => (StatusCode::NOT_FOUND, "not found".to_owned()),
            Self::Conflict(m) => (StatusCode::CONFLICT, m.clone()),
            Self::Chain(_) => {
                tracing::warn!(error = %self, "request needs the chain node");
                (StatusCode::SERVICE_UNAVAILABLE, "chain node unavailable".to_owned())
            }
            Self::Store(_) | Self::Internal(_) | Self::Config(_) => {
                tracing::error!(error = %self, "request failed internally");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error".to_owned())
            }
        };
        (status, axum::Json(serde_json::json!({ "error": message }))).into_response()
    }
}
