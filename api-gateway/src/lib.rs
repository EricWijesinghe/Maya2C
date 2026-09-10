//! REST and GraphQL gateway in front of a Maya2C node.
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
//! different host from the node it serves, and RocksDB's C++ stays out of the
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
pub mod error;
pub mod graphql;
pub mod node;
pub mod rest;
pub mod sealed;

use std::sync::Arc;

use async_graphql_axum::GraphQL;
use axum::Router;
use axum::routing::post_service;

use crate::node::NodeClient;
use crate::rest::AppState;

/// Builds the complete gateway router: REST plus GraphQL.
///
/// Both halves share one [`NodeClient`], and therefore one allowlist. A second
/// client would be a second policy.
pub fn app(node: Arc<dyn NodeClient>) -> Router {
    let schema = graphql::schema(Arc::clone(&node));
    let state = Arc::new(AppState { node });

    // `post_service`, not `post`: `GraphQL` is a tower `Service` rather than an
    // axum handler, and the two are not interchangeable.
    rest::router(state).route("/graphql", post_service(GraphQL::new(schema)))
}
