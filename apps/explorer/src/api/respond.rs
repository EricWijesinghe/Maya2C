//! Responses: JSON bodies with the cache policy and the X-headers.
//!
//! The cache policy is decided per response, not per route, because the same
//! route is immutable for an old block and stale-prone for a new one.

use axum::extract::Request;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::api::cache::Source;

/// Seconds a non-final response may be reused. Short because the tip moves.
const SHORT_MAX_AGE: u64 = 5;
/// A year: the longest span the spec's `immutable` is meant to cover.
const IMMUTABLE_MAX_AGE: u64 = 31_536_000;

/// `X-Indexed-Height`: highest height the index holds.
pub const INDEXED_HEIGHT: HeaderName = HeaderName::from_static("x-indexed-height");
/// `X-Tip-Height`: the node's tip when it last answered.
pub const TIP_HEIGHT: HeaderName = HeaderName::from_static("x-tip-height");
/// `X-Cache`: whether live data came from the cache.
pub const CACHE: HeaderName = HeaderName::from_static("x-cache");

/// How long a client may keep a response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freshness {
    /// Final data, at or below the finalized height. Never changes.
    Immutable,
    /// Data that a new block could still change.
    Short,
    /// Live node data: revalidate with the ETag every time.
    Live,
}

/// Immutable only at or below the finalized height. Without a finality
/// checkpoint nothing is known to be final.
#[must_use]
pub fn for_height(height: i64, finalized: Option<u64>) -> Freshness {
    match finalized {
        Some(final_height) if u64::try_from(height).is_ok_and(|h| h <= final_height) => {
            Freshness::Immutable
        }
        _ => Freshness::Short,
    }
}

/// The non-body parts of a response.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Meta {
    /// Highest indexed height, if the index has any block.
    pub indexed: Option<i64>,
    /// The node's tip, if it answered.
    pub tip: Option<u64>,
    /// Cache outcome, for live data.
    pub cache: Option<Source>,
}

/// A JSON response with its cache policy and X-headers.
#[must_use]
pub fn json<T: Serialize>(body: &T, freshness: Freshness, meta: Meta) -> Response {
    let bytes = match serde_json::to_vec(body) {
        Ok(bytes) => bytes,
        Err(error) => {
            // Our own DTOs always serialize; if one ever does not, that is a
            // server bug and must not reach the client as a partial body.
            eprintln!("api: serializing response: {error}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert(header::CACHE_CONTROL, cache_control(freshness));
    if freshness == Freshness::Live {
        let digest = Sha256::digest(&bytes);
        let etag = format!("\"{}\"", hex::encode(digest));
        if let Ok(value) = HeaderValue::from_str(&etag) {
            headers.insert(header::ETAG, value);
        }
    }
    if let Some(height) = meta.indexed {
        headers.insert(INDEXED_HEIGHT, HeaderValue::from(height));
    }
    if let Some(tip) = meta.tip {
        headers.insert(TIP_HEIGHT, HeaderValue::from(tip));
    }
    if let Some(source) = meta.cache {
        let word = match source {
            Source::Hit => "HIT",
            Source::Miss => "MISS",
        };
        headers.insert(CACHE, HeaderValue::from_static(word));
    }

    let mut response = (StatusCode::OK, bytes).into_response();
    response.headers_mut().extend(headers);
    response
}

fn cache_control(freshness: Freshness) -> HeaderValue {
    let text = match freshness {
        Freshness::Immutable => format!("public, max-age={IMMUTABLE_MAX_AGE}, immutable"),
        Freshness::Short => format!("public, max-age={SHORT_MAX_AGE}"),
        Freshness::Live => "no-cache".to_string(),
    };
    HeaderValue::from_str(&text).unwrap_or(HeaderValue::from_static("no-cache"))
}

/// Answers `If-None-Match` with 304 when the body's ETag matches.
pub async fn conditional(request: Request, next: Next) -> Response {
    let wanted = request.headers().get(header::IF_NONE_MATCH).cloned();
    let response = next.run(request).await;
    let Some(wanted) = wanted else {
        return response;
    };
    let matches = response
        .headers()
        .get(header::ETAG)
        .is_some_and(|etag| etag == &wanted);
    if response.status() != StatusCode::OK || !matches {
        return response;
    }
    let mut not_modified = StatusCode::NOT_MODIFIED.into_response();
    for name in [header::ETAG, header::CACHE_CONTROL] {
        if let Some(value) = response.headers().get(&name) {
            not_modified.headers_mut().insert(name, value.clone());
        }
    }
    not_modified
}

/// Marks the unversioned `/api/*` routes as deprecated in favour of `/api/v1`.
pub async fn legacy_api(request: Request, next: Next) -> Response {
    let path = request.uri().path().to_string();
    let mut response = next.run(request).await;
    if path.starts_with("/api/") && !path.starts_with("/api/v1/") {
        let headers = response.headers_mut();
        headers.insert(
            HeaderName::from_static("deprecation"),
            HeaderValue::from_static("true"),
        );
        headers.insert(
            header::LINK,
            HeaderValue::from_static("</api/v1/status>; rel=\"successor-version\""),
        );
    }
    response
}
