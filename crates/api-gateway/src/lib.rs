//! REST and GraphQL gateway in front of a `Maya2C` node.
//!
//! # What this is for
//!
//! The node speaks JSON-RPC on a port that assumes a trusted network position:
//! no authentication, and every method equally reachable — including the miner
//! interface. That is a sound design for something bound to a pod network and
//! an unsound one to expose publicly.
//!
//! This crate is the layer that makes a public interface possible: a
//! default-deny allowlist, size ceilings checked before allocation, GraphQL
//! depth and complexity limits, and errors that do not echo the backend's
//! internals to the caller.
//!
//! # It talks to a node over the network
//!
//! Not through `StateDB`. The gateway is a JSON-RPC client, so it can run on a
//! different host from the node it serves, and `RocksDB`'s C++ stays out of the
//! process that terminates untrusted HTTP. That is the same shape `explorer`
//! uses.
//!
//! # The three things it refuses to do
//!
//! 1. **Proxy the miner methods.** `get_mining_candidate` and `submit_block`
//!    are not on the allowlist and are named in [`allowlist::DENIED_METHODS`]
//!    so the exclusion has a test behind it.
//! 2. **Decrypt a sealed transaction.** It holds no committee share and cannot.
//!    A gateway that could read the threshold-encrypted mempool would be the
//!    observer that mempool exists to remove.
//! 3. **Expose an unbounded query.** Every GraphQL schema this crate builds
//!    carries a depth and a complexity limit; there is no constructor without
//!    them.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod allowlist;
pub mod coins;
pub mod error;
pub mod graphql;
pub mod jsonrpc;
pub mod limit;
pub mod node;
pub mod rest;
pub mod sealed;

use std::sync::Arc;

use async_graphql_axum::GraphQL;
use axum::Router;
use axum::routing::post_service;
use maya_web_security::{secure, security_txt};

use crate::node::NodeClient;
use crate::rest::AppState;

/// Builds the complete gateway router: REST plus GraphQL.
///
/// Both halves share one [`NodeClient`], and therefore one allowlist. A second
/// client would be a second policy.
pub fn app(node: Arc<dyn NodeClient>) -> Router {
    secure(routes(node), CONTENT_SECURITY_POLICY)
}

/// The gateway's routes, before the security headers go on.
fn routes(node: Arc<dyn NodeClient>) -> Router {
    let schema = graphql::schema(Arc::clone(&node));
    let state = Arc::new(AppState { node });

    // `post_service`, not `post`: `GraphQL` is a tower `Service` rather than an
    // axum handler, and the two are not interchangeable.
    rest::router(state)
        .route("/graphql", post_service(GraphQL::new(schema)))
        .merge(security_txt())
        .layer(cors())
}

/// JSON only, no HTML: nothing it serves can run script or be framed.
const CONTENT_SECURITY_POLICY: &str = "default-src 'none'; frame-ancestors 'none'";

/// [`app`] behind per-client rate limiting: what the public gateway serves.
///
/// # Errors
///
/// [`limit::InvalidRateLimit`] if the limits allow nothing.
pub fn app_limited(
    node: Arc<dyn NodeClient>,
    limits: &limit::RateLimit,
) -> Result<Router, limit::InvalidRateLimit> {
    Ok(app_with_limiter(node, &limit::config(limits)?))
}

/// [`app`] behind an existing limiter configuration: the binary passes the
/// same one to [`limit::spawn_pruning`], so the table it prunes is the one
/// this layer fills.
pub fn app_with_limiter(node: Arc<dyn NodeClient>, config: &limit::Config) -> Router {
    // Inside `secure`, so a 429 from the limiter carries the headers too.
    secure(
        routes(node).layer(tower_governor::GovernorLayer::new(Arc::clone(config))),
        CONTENT_SECURITY_POLICY,
    )
}

/// Lets browser apps (`maya2c.js`, the website's faucet and explorer pages)
/// call the gateway from a browser.
///
/// Defaults to any origin, which is acceptable only because the gateway has no
/// cookies, sessions or credentials, and everything it serves is public chain
/// data or a signed transaction the caller built. Set `MAYA2C_CORS_ALLOWED_ORIGINS`
/// to a comma-separated list to lock it to known front ends. Credentials stay
/// disallowed either way.
fn cors() -> tower_http::cors::CorsLayer {
    use axum::http::{Method, header};

    /// How long a browser may cache a preflight answer.
    const PREFLIGHT_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(3_600);

    maya_web_security::cors_from_env([Method::GET, Method::POST])
        .allow_headers([header::CONTENT_TYPE])
        .max_age(PREFLIGHT_MAX_AGE)
}
