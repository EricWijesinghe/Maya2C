//! `maya2c-mesh` — the Mesh Data API in front of a node.
//!
//! ```text
//! maya2c-mesh --node http://127.0.0.1:8545 --network maya-devnet --listen 127.0.0.1:8081
//! ```
//!
//! Binds loopback by default. It has no authentication or rate limiting of
//! its own, and every `/account/balance` at a past block is a walk over the
//! node's per-block changes: exposing it beyond the custodian's own network is
//! a deployment decision (TLS, auth and limits at the edge) this binary does
//! not make for anyone.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context as _;
use clap::Parser;
use maya_mesh_api::node::RpcNode;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(
    name = "maya2c-mesh",
    version,
    about = "Mesh Data API for a Maya2C node"
)]
struct Args {
    /// The node's JSON-RPC endpoint.
    #[arg(long, default_value = "http://127.0.0.1:8545", env = "MAYA_NODE_RPC")]
    node: String,
    /// The network name, the genesis `chain_id`.
    #[arg(long, env = "MAYA_MESH_NETWORK")]
    network: String,
    /// Address to serve on.
    #[arg(long, default_value = "127.0.0.1:8081", env = "MAYA_MESH_LISTEN")]
    listen: SocketAddr,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let args = Args::parse();
    let node = RpcNode::connect(&args.node).context("connecting to the node")?;
    let listener = tokio::net::TcpListener::bind(args.listen)
        .await
        .with_context(|| format!("binding {}", args.listen))?;
    tracing::info!(listen = %args.listen, network = %args.network, "mesh serving");
    axum::serve(
        listener,
        maya_mesh_api::router(Arc::new(node), &args.network),
    )
    .await
    .context("serving")
}
