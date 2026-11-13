//! RFC 9457 problem responses. Every v1 failure carries a stable `code`.

use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use utoipa::ToSchema;

/// The failures a client can act on. The strings are part of the v1 contract:
/// a new failure needs a new code, and an old one is never renamed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    /// The address is not a valid Bech32m address (hex is not accepted).
    InvalidAddress,
    /// The address is valid, but for another network.
    WrongNetwork,
    /// The cursor was not issued by this API.
    InvalidCursor,
    /// A query parameter is malformed or out of range.
    InvalidParam,
    /// Nothing is indexed or known under that reference.
    NotFound,
    /// The client exceeded its request rate.
    RateLimited,
    /// The node could not answer a live query.
    NodeUnavailable,
    /// The index is not built yet, so history cannot be answered.
    IndexBehind,
    /// The network has no staking, so there is no validator set.
    NoStaking,
    /// The server could not handle the request; not the client's fault.
    Internal,
}

impl Code {
    /// HTTP status for this failure.
    #[must_use]
    pub const fn status(self) -> StatusCode {
        match self {
            Self::InvalidAddress
            | Self::WrongNetwork
            | Self::InvalidCursor
            | Self::InvalidParam => StatusCode::BAD_REQUEST,
            Self::NotFound | Self::NoStaking => StatusCode::NOT_FOUND,
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Self::NodeUnavailable | Self::IndexBehind => StatusCode::SERVICE_UNAVAILABLE,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// The wire name, identical to the serde rename.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidAddress => "invalid_address",
            Self::WrongNetwork => "wrong_network",
            Self::InvalidCursor => "invalid_cursor",
            Self::InvalidParam => "invalid_param",
            Self::NotFound => "not_found",
            Self::RateLimited => "rate_limited",
            Self::NodeUnavailable => "node_unavailable",
            Self::IndexBehind => "index_behind",
            Self::NoStaking => "no_staking",
            Self::Internal => "internal",
        }
    }
}

/// A failed v1 request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiError {
    /// What went wrong, for a program to branch on.
    pub code: Code,
    /// Human-readable detail. Never a stack trace or an internal address.
    pub detail: String,
}

impl ApiError {
    /// An error with `code` and `detail`.
    #[must_use]
    pub fn new(code: Code, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

/// Builds a `application/problem+json` response.
///
/// `extra` carries headers the failure needs, such as `Retry-After` on a 429.
#[must_use]
pub fn problem_response(code: Code, detail: &str, extra: HeaderMap) -> Response {
    let status = code.status();
    let body = serde_json::json!({
        "type": format!("urn:maya2c:error:{}", code.as_str()),
        "title": status.canonical_reason().unwrap_or("Error"),
        "status": status.as_u16(),
        "detail": detail,
        "code": code.as_str(),
    });
    let mut response = (status, body.to_string()).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/problem+json"),
    );
    headers.extend(extra);
    response
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        problem_response(self.code, &self.detail, HeaderMap::new())
    }
}
