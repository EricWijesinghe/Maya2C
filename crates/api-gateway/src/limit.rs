//! Per-client rate limiting for the public gateway.
//!
//! A token bucket per client address (`governor`, through `tower_governor`):
//! `burst` requests at once, refilled at `per_second`. It is the only thing
//! between one client and a flood of node calls, so the part that matters most
//! is not the arithmetic but **who counts as a client**.
//!
//! # Identifying the client
//!
//! Exposed directly, the socket's peer address is the client ([`ClientIp::Socket`]),
//! and any header is whatever the client chose to write.
//!
//! Behind a proxy every request arrives from the proxy, so the client comes
//! from a header the proxy sets:
//!
//! - [`ClientIp::CfConnectingIp`]: Cloudflare's edge **overwrites**
//!   `CF-Connecting-IP` with the visitor's address. That is how
//!   `maya-testnet-1` is published (a Cloudflare Tunnel to loopback).
//! - [`ClientIp::XForwardedForLast`]: the **last** entry of
//!   `X-Forwarded-For`, which the proxy in front appended. A client can
//!   prepend anything; keying on the first entry would let it rotate
//!   addresses at will.
//!
//! A header mode is only sound when the proxy is the sole route to the
//! gateway, which is why the gateway binds 127.0.0.1 by default. A request
//! that arrives without the header (a local health check) falls back to its
//! socket address rather than failing.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ConnectInfo;
use axum::http::Request;
use governor::middleware::NoOpMiddleware;
use tower_governor::GovernorError;
use tower_governor::governor::{GovernorConfig, GovernorConfigBuilder};
use tower_governor::key_extractor::KeyExtractor;

/// How often forgotten clients are pruned from the per-address table.
///
/// Without pruning, every address that ever called would keep an entry, so
/// the limiter would itself become a slow memory exhaustion.
pub const PRUNE_INTERVAL: Duration = Duration::from_secs(60);

/// Where the client's address comes from. See the module docs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientIp {
    /// The TCP peer. For a gateway exposed directly.
    Socket,
    /// `CF-Connecting-IP`, set by Cloudflare's edge.
    CfConnectingIp,
    /// The last entry of `X-Forwarded-For`, appended by the proxy in front.
    XForwardedForLast,
}

/// Rate-limit settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RateLimit {
    /// Sustained requests per second, per client.
    pub per_second: u64,
    /// Requests a client may make at once before the rate applies.
    pub burst: u32,
    /// How the client is identified.
    pub client_ip: ClientIp,
}

/// Why rate-limit settings were refused.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("rate limit must allow at least one request per second and a burst of at least one")]
pub struct InvalidRateLimit;

/// The client key: the configured [`ClientIp`] source.
#[derive(Clone, Copy, Debug)]
pub struct ClientKey(ClientIp);

impl KeyExtractor for ClientKey {
    type Key = IpAddr;

    fn extract<T>(&self, request: &Request<T>) -> Result<IpAddr, GovernorError> {
        let from_header = match self.0 {
            ClientIp::Socket => None,
            ClientIp::CfConnectingIp => header_ip(request, "cf-connecting-ip", |value| Some(value)),
            ClientIp::XForwardedForLast => {
                header_ip(request, "x-forwarded-for", |value| value.rsplit(',').next())
            }
        };
        from_header
            .or_else(|| {
                request
                    .extensions()
                    .get::<ConnectInfo<SocketAddr>>()
                    .map(|ConnectInfo(peer)| peer.ip())
            })
            .ok_or(GovernorError::UnableToExtractKey)
    }
}

fn header_ip<T>(
    request: &Request<T>,
    name: &str,
    pick: impl Fn(&str) -> Option<&str>,
) -> Option<IpAddr> {
    let value = request.headers().get(name)?.to_str().ok()?;
    pick(value)?.trim().parse().ok()
}

/// The limiter's configuration, shared by the layer and the pruning task.
pub type Config = Arc<GovernorConfig<ClientKey, NoOpMiddleware<governor::clock::QuantaInstant>>>;

/// Builds the limiter configuration.
///
/// # Errors
///
/// [`InvalidRateLimit`] if `per_second` or `burst` is zero.
pub fn config(limits: &RateLimit) -> Result<Config, InvalidRateLimit> {
    if limits.per_second == 0 || limits.burst == 0 {
        return Err(InvalidRateLimit);
    }
    // governor refills one token per period; a rate above 1,000/s would
    // round the period to zero, so it is floored at one millisecond.
    let period_ms = (1_000 / limits.per_second).max(1);
    GovernorConfigBuilder::default()
        .per_millisecond(period_ms)
        .burst_size(limits.burst)
        .key_extractor(ClientKey(limits.client_ip))
        .finish()
        .map(Arc::new)
        .ok_or(InvalidRateLimit)
}

/// Prunes clients that have not called recently, every [`PRUNE_INTERVAL`].
///
/// Spawned once by the binary; tests do not need it.
pub fn spawn_pruning(config: &Config) -> tokio::task::JoinHandle<()> {
    let limiter = Arc::clone(config.limiter());
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(PRUNE_INTERVAL);
        loop {
            tick.tick().await;
            limiter.retain_recent();
        }
    })
}
