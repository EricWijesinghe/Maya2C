//! The HTTP surface: three routes and the status codes they mean.
//!
//! | Route | Purpose |
//! |---|---|
//! | `GET /health` | liveness, no state |
//! | `GET /status` | chain id, dispense size, budget left |
//! | `POST /request` | the grant |
//!
//! # The client's IP is not the socket's IP
//!
//! Behind a load balancer every request arrives from the balancer, so keying
//! the limiter on the peer address would give the whole internet one shared
//! bucket — the first request of the day would be granted and every other
//! refused. The forwarded header is therefore trusted, and that trust is only
//! safe because [`ClientIp`] takes the **last** hop rather than the first: a
//! client can append entries to `X-Forwarded-For`, but it cannot remove the one
//! the proxy in front of the faucet appends. Anything to the left of that is
//! the client's own claim.
//!
//! This is why [`FaucetService::trust_proxy`] exists and defaults to off. A
//! faucet exposed directly must not read the header at all, because there
//! nothing appends it and the value is entirely the caller's.
//!
//! # A refusal is not a failure
//!
//! `429` for a rate limit, `503` for an exhausted budget, `400` for a bad
//! address. Only a node or signing problem is a `500`, so an alert on 5xx
//! means the faucet is broken rather than merely busy.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::Json;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Router, extract::FromRequestParts};

use crate::dispense::{DispenseError, Dispenser};
use crate::limit::Refusal;
use crate::{Faucet, FaucetError};

/// Header a reverse proxy appends the client's address to.
pub const FORWARDED_FOR: &str = "x-forwarded-for";

/// Largest request body accepted.
///
/// A grant request is one JSON object holding a 64-character address. Anything
/// larger is not that, and reading it would be free memory for an attacker.
pub const MAX_BODY_BYTES: usize = 1024;

/// The faucet, its dispenser, and how it learns a caller's address.
pub struct FaucetService {
    faucet: Faucet,
    dispenser: Arc<dyn Dispenser>,
    /// Whether to read [`FORWARDED_FOR`].
    ///
    /// Off by default. See the module documentation: a faucet with no proxy in
    /// front of it that trusts this header lets any caller pick their own
    /// rate-limit bucket, which is the same as having no limit.
    trust_proxy: bool,
}

impl FaucetService {
    /// Builds a service that keys on the socket's peer address.
    pub fn new(faucet: Faucet, dispenser: Arc<dyn Dispenser>) -> Self {
        Self {
            faucet,
            dispenser,
            trust_proxy: false,
        }
    }

    /// Reads the client address from [`FORWARDED_FOR`] instead.
    ///
    /// Only correct when a proxy that appends the header is the *only* way to
    /// reach this service. If the port is also reachable directly, the direct
    /// path has no rate limit at all.
    #[must_use]
    pub fn trust_proxy(mut self, trust: bool) -> Self {
        self.trust_proxy = trust;
        self
    }

    /// Whether the forwarded header is honoured.
    #[must_use]
    pub const fn trusts_proxy(&self) -> bool {
        self.trust_proxy
    }
}

/// The routes, ready to serve.
pub fn router(service: Arc<FaucetService>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/status", get(status))
        .route("/request", post(request))
        .layer(tower_http::limit::RequestBodyLimitLayer::new(
            MAX_BODY_BYTES,
        ))
        .with_state(service)
}

/// What a caller asks for.
#[derive(Debug, serde::Deserialize)]
pub struct GrantRequest {
    /// Hex-encoded 32-byte address, with or without a `0x` prefix.
    pub address: String,
}

/// What the faucet is willing to say about itself.
#[derive(Debug, serde::Serialize)]
pub struct Status {
    /// The chain being funded.
    pub chain_id: String,
    /// Units per grant.
    pub dispense: u64,
    /// The day's ceiling.
    pub daily_cap: u64,
    /// Units already handed out today.
    pub spent_today: u64,
    /// Seconds one grant blocks the next.
    pub window_seconds: u64,
}

/// An error, as a caller sees it.
#[derive(Debug, serde::Serialize)]
pub struct ErrorBody {
    /// A short machine-readable tag.
    pub error: &'static str,
    /// A sentence a person can act on.
    pub message: String,
    /// Seconds to wait, when waiting is the remedy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after: Option<u64>,
}

async fn health() -> &'static str {
    "ok"
}

async fn status(State(service): State<Arc<FaucetService>>) -> Json<Status> {
    Json(Status {
        chain_id: service.faucet.chain_id().to_string(),
        dispense: service.faucet.dispense(),
        daily_cap: service.faucet.daily_cap(),
        spent_today: service.faucet.spent_today(),
        window_seconds: crate::limit::WINDOW.as_secs(),
    })
}

async fn request(
    State(service): State<Arc<FaucetService>>,
    ClientIp(ip): ClientIp,
    Json(body): Json<GrantRequest>,
) -> Response {
    let grant = match service
        .faucet
        .request(ip, &body.address, std::time::SystemTime::now())
    {
        Ok(grant) => grant,
        Err(error) => return refusal(&error).into_response(),
    };

    // The recipient parsed cleanly inside `request`, or it would not have
    // granted. Re-parsing rather than threading the bytes back out keeps the
    // policy's signature in strings, which is what the HTTP layer has.
    let recipient = match crate::parse_address(&body.address) {
        Ok(bytes) => bytes,
        Err(error) => return refusal(&error).into_response(),
    };

    match service.dispenser.send(&recipient, grant.amount).await {
        Ok(txid) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "txid": txid,
                "amount": grant.amount,
                "remaining_today": grant.remaining_today,
                "next_request_in": grant.next_request_in,
            })),
        )
            .into_response(),

        // The grant has already been recorded against both windows, and it is
        // not given back. That is deliberate: a submission failure the caller
        // could retry freely would be a way to spend the faucet's budget
        // without ever completing a grant. The operator sees the 5xx.
        Err(error) => {
            tracing::error!(%error, "dispensing failed after the grant was recorded");
            let (status, tag) = match error {
                DispenseError::Underfunded => (StatusCode::SERVICE_UNAVAILABLE, "underfunded"),
                _ => (StatusCode::INTERNAL_SERVER_ERROR, "dispense_failed"),
            };
            (
                status,
                Json(ErrorBody {
                    error: tag,
                    // Deliberately not `error`: a node URL or an internal path
                    // in a public response is a free map of the deployment.
                    message: "The faucet could not send the transaction. Please try again \
                              later."
                        .to_string(),
                    retry_after: None,
                }),
            )
                .into_response()
        }
    }
}

/// Maps a policy refusal to a status code and a body.
fn refusal(error: &FaucetError) -> (StatusCode, Json<ErrorBody>) {
    let (status, tag, retry_after) = match error {
        FaucetError::RateLimited(refusal) => (
            StatusCode::TOO_MANY_REQUESTS,
            match refusal {
                Refusal::Ip { .. } => "rate_limited_ip",
                Refusal::Address { .. } => "rate_limited_address",
            },
            Some(refusal.retry_after()),
        ),
        FaucetError::BudgetExhausted => (StatusCode::SERVICE_UNAVAILABLE, "budget_exhausted", None),
        FaucetError::BadAddress(_) => (StatusCode::BAD_REQUEST, "bad_address", None),
        // Unreachable through this handler: a value-bearing chain is refused at
        // construction, so no service exists to serve a request. Mapped anyway
        // rather than left to a catch-all, so adding a per-request check later
        // cannot silently turn it into a 500.
        FaucetError::ValueBearingChain(_) => (StatusCode::FORBIDDEN, "disabled", None),
    };

    (
        status,
        Json(ErrorBody {
            error: tag,
            message: error.to_string(),
            retry_after,
        }),
    )
}

/// The caller's address, from the socket or the trusted proxy header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientIp(pub IpAddr);

impl FromRequestParts<Arc<FaucetService>> for ClientIp {
    type Rejection = (StatusCode, Json<ErrorBody>);

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &Arc<FaucetService>,
    ) -> Result<Self, Self::Rejection> {
        if state.trust_proxy
            && let Some(ip) = forwarded_client_ip(&parts.headers)
        {
            return Ok(Self(ip));
        }

        parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(addr)| Self(addr.ip()))
            .ok_or_else(|| {
                // No peer address and no trusted header means there is no key
                // to limit on, and serving the request would be serving it
                // unlimited. Refusing is the only safe answer.
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorBody {
                        error: "no_client_address",
                        message: "The faucet could not determine the client address.".to_string(),
                        retry_after: None,
                    }),
                )
            })
    }
}

/// The **last** entry of `X-Forwarded-For`.
///
/// Last, not first. A client may send the header itself, and every value it
/// sends lands to the left of whatever the proxy in front of the faucet
/// appends. Taking the first entry would let any caller name their own bucket.
fn forwarded_client_ip(headers: &HeaderMap) -> Option<IpAddr> {
    headers
        .get(FORWARDED_FOR)?
        .to_str()
        .ok()?
        .rsplit(',')
        .next()?
        .trim()
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use axum::http::HeaderValue;

    fn headers(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(FORWARDED_FOR, HeaderValue::from_str(value).expect("header"));
        headers
    }

    #[test]
    fn the_forwarded_address_is_the_last_hop() {
        // The one that matters. A caller who sends
        // `X-Forwarded-For: 1.1.1.1` gets that value prepended to what the
        // proxy appends, so the first entry is theirs and the last is real.
        let ip = forwarded_client_ip(&headers("1.1.1.1, 203.0.113.9")).expect("parsed");
        assert_eq!(ip, "203.0.113.9".parse::<IpAddr>().expect("ip"));
    }

    #[test]
    fn a_single_entry_is_used_as_is() {
        let ip = forwarded_client_ip(&headers("203.0.113.4")).expect("parsed");
        assert_eq!(ip, "203.0.113.4".parse::<IpAddr>().expect("ip"));
    }

    #[test]
    fn an_ipv6_forwarded_address_parses() {
        let ip = forwarded_client_ip(&headers("2001:db8::1")).expect("parsed");
        assert_eq!(ip, "2001:db8::1".parse::<IpAddr>().expect("ip"));
    }

    #[test]
    fn a_junk_header_yields_nothing_rather_than_a_default() {
        // Falling back to the peer address is right; inventing an address —
        // `0.0.0.0`, say — would put every malformed request in one bucket.
        for junk in ["", "not-an-ip", "1.1.1.1, ", "999.999.999.999"] {
            assert!(forwarded_client_ip(&headers(junk)).is_none(), "{junk}");
        }
    }

    #[test]
    fn the_proxy_header_is_not_trusted_by_default() {
        let faucet = Faucet::new("maya-genesis-rc1", 100, 1_000, std::time::SystemTime::now())
            .expect("testnet");
        let service = FaucetService::new(faucet, Arc::new(crate::testing::NullDispenser));
        assert!(
            !service.trusts_proxy(),
            "a directly exposed faucet that trusted this header would have no limit at all"
        );
    }
}
