//! `maya2c-gateway` — serves [`maya_api_gateway::app`] in front of a node.
//!
//! ```text
//! maya2c-gateway --node http://127.0.0.1:8545 --listen 127.0.0.1:8080
//! ```
//!
//! Binds loopback by default. Exposing it publicly is a deployment decision
//! (TLS, rate limits at the edge) that this binary does not make for anyone.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context as _;
use clap::Parser;
use maya_api_gateway::node::RpcNodeClient;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(
    name = "maya2c-gateway",
    version,
    about = "REST and GraphQL gateway for a Maya2C node"
)]
struct Args {
    /// The node's JSON-RPC endpoint.
    #[arg(long, default_value = "http://127.0.0.1:8545", env = "MAYA_NODE_RPC")]
    node: String,
    /// Address to serve on.
    #[arg(long, default_value = "127.0.0.1:8080", env = "MAYA_GATEWAY_LISTEN")]
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
    let node = RpcNodeClient::connect(&args.node).context("connecting to the node")?;
    let listener = tokio::net::TcpListener::bind(args.listen)
        .await
        .with_context(|| format!("binding {}", args.listen))?;
    tracing::info!(listen = %args.listen, node = %args.node, "gateway serving");
    axum::serve(listener, maya_api_gateway::app(Arc::new(node)))
        .with_graceful_shutdown(async {
            // A failed signal handler means no graceful stop, not no gateway.
            if let Err(error) = tokio::signal::ctrl_c().await {
                tracing::warn!(%error, "ctrl-c handler unavailable");
                std::future::pending::<()>().await;
            }
        })
        .await
        .context("serving")
}
