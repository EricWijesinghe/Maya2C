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

use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ConnectInfo;
use axum::http::{Extensions, HeaderMap, Request};
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
        client_address(self.0, request.headers(), request.extensions())
            .ok_or(GovernorError::UnableToExtractKey)
    }
}

/// The client's address as the limiter keys it, for anything else that must
/// count per client the same way (the gateway's event subscription caps).
#[must_use]
pub fn client_address(
    mode: ClientIp,
    headers: &HeaderMap,
    extensions: &Extensions,
) -> Option<IpAddr> {
    let from_header = match mode {
        ClientIp::Socket => None,
        ClientIp::CfConnectingIp => header_ip(headers, "cf-connecting-ip", |value| Some(value)),
        ClientIp::XForwardedForLast => {
            header_ip(headers, "x-forwarded-for", |value| value.rsplit(',').next())
        }
    };
    from_header
        .or_else(|| {
            extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|ConnectInfo(peer)| peer.ip())
        })
        .map(bucket_of)
}

/// An IPv6 client counts as its /64: one subscriber is normally handed a
/// whole /64, so keying on the full address would give it 2^64 buckets to
/// rotate through. An IPv4-mapped address (`::ffff:a.b.c.d`) stays the IPv4
/// address it is, or every IPv4 client of a dual-stack socket would share one.
fn bucket_of(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(Ipv6Addr::from(u128::from(v6) & !u128::from(u64::MAX))),
        },
        IpAddr::V4(_) => ip,
    }
}

fn header_ip(
    headers: &HeaderMap,
    name: &str,
    pick: impl Fn(&str) -> Option<&str>,
) -> Option<IpAddr> {
    // The last header line, not the first: a client can send its own
    // `X-Forwarded-For` line before the proxy's, and the proxy's line is the
    // one appended last (several lines are one comma-joined list, RFC 9110 5.3).
    let value = headers.get_all(name).iter().next_back()?.to_str().ok()?;
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
    tokio::spawn(prune(Arc::clone(config)))
}

/// The pruning loop itself, for a caller that runs it inside its own task
/// (so aborting that task stops it too). Never returns.
pub async fn prune(config: Config) {
    let mut tick = tokio::time::interval(PRUNE_INTERVAL);
    loop {
        tick.tick().await;
        config.limiter().retain_recent();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_with(lines: &[&str]) -> Request<()> {
        let mut builder = Request::builder();
        for line in lines {
            builder = builder.header("x-forwarded-for", *line);
        }
        builder.body(()).expect("request")
    }

    #[test]
    fn a_second_forwarded_for_line_cannot_hide_the_proxys_hop() {
        let key = ClientKey(ClientIp::XForwardedForLast);
        let honest = request_with(&["203.0.113.9"]);
        let spoofed = request_with(&["1.1.1.1", "203.0.113.9"]);
        let rotated = request_with(&["2.2.2.2", "203.0.113.9"]);
        let expected: IpAddr = "203.0.113.9".parse().expect("ip");
        assert_eq!(key.extract(&honest).expect("key"), expected);
        assert_eq!(key.extract(&spoofed).expect("key"), expected);
        assert_eq!(key.extract(&rotated).expect("key"), expected);
    }

    fn key_of(ip: &str) -> IpAddr {
        let mut headers = HeaderMap::new();
        headers.insert("cf-connecting-ip", ip.parse().expect("header"));
        client_address(ClientIp::CfConnectingIp, &headers, &Extensions::new()).expect("key")
    }

    #[test]
    fn ipv6_clients_in_one_slash_64_share_a_bucket() {
        assert_eq!(
            key_of("2001:db8:1:2::1"),
            key_of("2001:db8:1:2:ffff:abcd::9")
        );
        assert_ne!(key_of("2001:db8:1:2::1"), key_of("2001:db8:1:3::1"));
    }

    #[test]
    fn ipv4_keys_are_untouched_even_when_mapped_into_ipv6() {
        assert_ne!(key_of("::ffff:198.51.100.1"), key_of("::ffff:198.51.100.2"));
        assert_eq!(key_of("::ffff:198.51.100.1"), key_of("198.51.100.1"));
        assert_ne!(key_of("198.51.100.1"), key_of("198.51.100.2"));
    }
}
