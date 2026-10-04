//! `maya2c-gateway` — serves [`maya_api_gateway::app`] in front of a node.
//!
//! ```text
//! maya2c-gateway --node http://127.0.0.1:8545 --listen 127.0.0.1:8080
//! ```
//!
//! Binds loopback by default. Exposing it publicly is a deployment decision
//! (TLS at the edge) that this binary does not make for anyone. Per-client
//! rate limits are on by default; behind a proxy, `--client-ip` must name the
//! header that proxy sets, or every visitor shares the proxy's one budget.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context as _;
use clap::Parser;
use maya_api_gateway::limit::{self, ClientIp, RateLimit};
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
    /// Sustained requests per second allowed per client.
    #[arg(long, default_value_t = 20, env = "MAYA_GATEWAY_RATE")]
    rate_per_second: u64,
    /// Requests a client may make at once before the rate applies.
    #[arg(long, default_value_t = 40, env = "MAYA_GATEWAY_BURST")]
    rate_burst: u32,
    /// How clients are identified for rate limiting: the TCP peer, or the
    /// header a proxy in front sets. See `limit` for why this matters.
    #[arg(long, value_enum, default_value_t = ClientIpArg::Socket, env = "MAYA_GATEWAY_CLIENT_IP")]
    client_ip: ClientIpArg,
    /// Also forward the pruned-node bootstrap methods (snapshots, headers),
    /// for a separate instance new validators restore from. Off on the
    /// public gateway.
    #[arg(long, env = "MAYA_GATEWAY_BOOTSTRAP")]
    bootstrap: bool,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum ClientIpArg {
    /// The TCP peer address: the gateway is exposed directly.
    Socket,
    /// `CF-Connecting-IP`: behind Cloudflare (proxy or tunnel).
    CfConnectingIp,
    /// The last `X-Forwarded-For` entry: behind another reverse proxy.
    XForwardedForLast,
}

impl From<ClientIpArg> for ClientIp {
    fn from(arg: ClientIpArg) -> Self {
        match arg {
            ClientIpArg::Socket => Self::Socket,
            ClientIpArg::CfConnectingIp => Self::CfConnectingIp,
            ClientIpArg::XForwardedForLast => Self::XForwardedForLast,
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let args = Args::parse();
    let node = RpcNodeClient::connect(&args.node)
        .context("connecting to the node")?
        .with_bootstrap(args.bootstrap);
    let listener = tokio::net::TcpListener::bind(args.listen)
        .await
        .with_context(|| format!("binding {}", args.listen))?;
    let limits = RateLimit {
        per_second: args.rate_per_second,
        burst: args.rate_burst,
        client_ip: args.client_ip.into(),
    };
    let limiter = limit::config(&limits).context("rate limit settings")?;
    let app = maya_api_gateway::app_with_limiter(Arc::new(node), &limiter);
    limit::spawn_pruning(&limiter);
    tracing::info!(
        listen = %args.listen, node = %args.node, ?limits, "gateway serving"
    );
    // Connect info gives the limiter the peer address in socket mode, and a
    // fallback key for a request that arrived without the proxy header.
    let service = app.into_make_service_with_connect_info::<SocketAddr>();
    axum::serve(listener, service)
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
