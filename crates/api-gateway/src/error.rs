//! What the gateway returns when it cannot answer.
//!
//! # Errors do not carry upstream detail to the client
//!
//! [`GatewayError::Upstream`] holds the node's message for the log and renders
//! as a flat "upstream node error" to the caller. A gateway that echoed the
//! backend's error text would leak the node's internal addresses, `RocksDB`
//! paths, and version strings to anyone who could provoke a failure.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// Why a gateway request failed.
#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    /// A method not on the allowlist was requested.
    ///
    /// Rendered as 404 rather than 403: a public gateway should not confirm
    /// that a method it refuses to serve exists on the node behind it.
    #[error("method not available: {0}")]
    MethodNotAllowed(String),

    /// The request was malformed — a bad address, a non-numeric height.
    #[error("{0}")]
    BadRequest(String),

    /// The node could not be reached, or failed internally.
    #[error("upstream node error: {0}")]
    Upstream(String),

    /// The node was reached and refused the request itself — a bad nonce, an
    /// invalid signature, malformed hex. The caller's fault, so 400 rather
    /// than 502; the node's text stays in the log, as for [`Self::Upstream`].
    #[error("rejected by the node: {0}")]
    Rejected(String),

    /// A submitted payload exceeded its size ceiling.
    #[error("payload too large: {0}")]
    PayloadTooLarge(String),

    /// The requested object does not exist.
    #[error("not found")]
    NotFound,
}

impl GatewayError {
    /// HTTP status for this failure.
    #[must_use]
    pub const fn status(&self) -> StatusCode {
        match self {
            Self::MethodNotAllowed(_) | Self::NotFound => StatusCode::NOT_FOUND,
            Self::BadRequest(_) | Self::Rejected(_) => StatusCode::BAD_REQUEST,
            Self::PayloadTooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            Self::Upstream(_) => StatusCode::BAD_GATEWAY,
        }
    }

    /// The message a *client* is allowed to see.
    ///
    /// Distinct from [`std::fmt::Display`], which is what goes in the log. The
    /// upstream variant is deliberately flattened: the node's error text names
    /// internal hosts and paths.
    #[must_use]
    pub fn public_message(&self) -> String {
        match self {
            Self::Upstream(_) => "upstream node error".to_string(),
            Self::Rejected(_) => "rejected by the node".to_string(),
            other => other.to_string(),
        }
    }
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        // Logged in full, returned in part.
        match &self {
            Self::Upstream(_) => tracing::warn!(error = %self, "upstream failure"),
            Self::Rejected(_) => tracing::info!(error = %self, "request refused by the node"),
            _ => {}
        }
        let body = Json(serde_json::json!({ "error": self.public_message() }));
        (self.status(), body).into_response()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn upstream_detail_is_not_shown_to_the_client() {
        // The whole point of `public_message`. If this ever echoed the inner
        // string, a client could learn the node's address by provoking a
        // connection error.
        let error = GatewayError::Upstream(
            "connecting to node at http://10.0.3.7:8545/: connection refused".to_string(),
        );
        let public = error.public_message();

        assert_eq!(public, "upstream node error");
        assert!(!public.contains("10.0.3.7"));
        assert!(!public.contains("8545"));
    }

    #[test]
    fn a_refused_method_looks_like_a_missing_one() {
        // 404, not 403: the gateway does not confirm that a method it refuses
        // to serve exists on the node behind it.
        let error = GatewayError::MethodNotAllowed("submit_block".to_string());
        assert_eq!(error.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn every_variant_maps_to_a_sensible_status() {
        assert_eq!(
            GatewayError::BadRequest("bad".into()).status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            GatewayError::PayloadTooLarge("big".into()).status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(
            GatewayError::Upstream("x".into()).status(),
            StatusCode::BAD_GATEWAY
        );
        assert_eq!(GatewayError::NotFound.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            GatewayError::Rejected("x".into()).status(),
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn a_refusal_is_the_callers_fault_but_hides_the_nodes_text() {
        let error =
            GatewayError::Rejected("invalid nonce for a4fe…: expected 1, got 0".to_string());
        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
        assert_eq!(error.public_message(), "rejected by the node");
    }
}
