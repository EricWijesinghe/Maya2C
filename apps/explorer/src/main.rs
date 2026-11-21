//! Explorer daemon.
//!
//! ```text
//! explorer --node-rpc http://127.0.0.1:8545 \
//!          --listen 0.0.0.0:3000 \
//!          --database-url postgres://user:pass@localhost/maya
//! ```
//!
//! With no `--database-url` the explorer runs against an in-memory index. That
//! is genuinely useful for a local chain, and it is what makes the explorer
//! runnable with no database to hand — but the index is lost on restart and
//! reindexed from genesis.

use std::error::Error;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use maya_explorer::indexer::{Indexer, RpcSource};
use maya_explorer::server::{AppState, fetch_network, serve};
use maya_explorer::store::BlockStore;
use maya_explorer::store::memory::MemoryStore;
use maya_explorer::store::postgres::PostgresStore;
use maya_web_security::rate_limit::ClientIp;

struct Args {
    node_rpc: String,
    listen: SocketAddr,
    database_url: Option<String>,
    max_connections: u32,
    /// Directory holding the branding assets.
    ///
    /// A flag rather than a compiled-in path, because this resolves against the
    /// process's working directory. The explorer is a standalone binary — the
    /// root `infra/docker/Dockerfile` packages only `node`, `genesis` and `peerid` — so a
    /// hardcoded relative path would serve silent 404s for every icon the
    /// moment somebody started it from anywhere but the repository root, with
    /// nothing in the logs to say why.
    assets: PathBuf,
    /// How `/api/v1` identifies clients for rate limiting (ADR-092).
    api_client_ip: ClientIp,
    /// `--trust-proxy-header`: a header mode on a non-loopback listen address
    /// is accepted. The limiter still believes the header only from a
    /// loopback peer (a proxy on this host); a remote proxy is one client.
    trust_proxy_header: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            node_rpc: "http://127.0.0.1:8545".to_string(),
            listen: "0.0.0.0:3000".parse().expect("valid default address"),
            database_url: std::env::var("DATABASE_URL").ok(),
            max_connections: 5,
            assets: PathBuf::from("assets"),
            api_client_ip: ClientIp::Socket,
            trust_proxy_header: false,
        }
    }
}

/// Parses `--api-client-ip`.
fn client_ip(text: &str) -> Result<ClientIp, Box<dyn Error>> {
    match text {
        "socket" => Ok(ClientIp::Socket),
        "cf" => Ok(ClientIp::CfConnectingIp),
        "xff" => Ok(ClientIp::XForwardedForLast),
        other => Err(format!("--api-client-ip must be socket, cf or xff, not {other}").into()),
    }
}

/// Refuses a header mode on a public address unless asked. The limiter believes
/// `CF-Connecting-IP` / `X-Forwarded-For` only from a loopback peer, so a
/// direct client cannot pick its own key; but a proxy on another host would
/// silently put every visitor in one bucket, which is worth a refusal.
fn check_client_ip(listen: SocketAddr, mode: ClientIp, trusted: bool) -> Result<(), String> {
    if mode == ClientIp::Socket || listen.ip().is_loopback() || trusted {
        return Ok(());
    }
    Err(format!(
        "--api-client-ip cf/xff on {listen}: the header is believed only from a loopback \
         peer (a proxy on this host), so a proxy elsewhere counts as one client. Listen \
         on 127.0.0.1 behind the proxy, or pass --trust-proxy-header to accept this."
    ))
}

/// Socket mode on loopback: every visitor arrives from the proxy on this host,
/// so the whole public API would share one client's rate limit.
fn loopback_socket_warning(listen: SocketAddr, mode: ClientIp) -> Option<String> {
    (mode == ClientIp::Socket && listen.ip().is_loopback()).then(|| {
        format!(
            "api:    listening on {listen} with --api-client-ip socket: behind a proxy every \
             visitor shares one rate-limit bucket; pass --api-client-ip cf or xff"
        )
    })
}

fn print_usage() {
    println!(
        "explorer — Maya2C block explorer\n\n\
         USAGE:\n  \
         explorer [OPTIONS]\n\n\
         OPTIONS:\n  \
         --node-rpc <URL>       node JSON-RPC endpoint (default http://127.0.0.1:8545)\n  \
         --listen <ADDR>        HTTP listen address (default 0.0.0.0:3000)\n  \
         --database-url <URL>   PostgreSQL URL; falls back to $DATABASE_URL,\n                         \
         then to an in-memory index\n  \
         --max-connections <N>  database pool size (default 5)\n  \
         --assets <DIR>         branding assets directory (default ./assets)\n  \
         --api-client-ip <MODE> /api/v1 client identity: socket (default), cf, xff\n  \
         --trust-proxy-header   allow cf/xff on a non-loopback --listen address\n  \
         -h, --help             show this message"
    );
}

fn parse_args() -> Result<Args, Box<dyn Error>> {
    let mut args = Args::default();
    let mut argv = std::env::args().skip(1);

    while let Some(flag) = argv.next() {
        let mut value = || {
            argv.next()
                .ok_or_else(|| format!("{flag} requires a value"))
        };
        match flag.as_str() {
            "--node-rpc" => args.node_rpc = value()?,
            "--listen" => args.listen = value()?.parse()?,
            "--database-url" => args.database_url = Some(value()?),
            "--max-connections" => args.max_connections = value()?.parse()?,
            "--assets" => args.assets = PathBuf::from(value()?),
            "--api-client-ip" => args.api_client_ip = client_ip(&value()?)?,
            "--trust-proxy-header" => args.trust_proxy_header = true,
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    check_client_ip(args.listen, args.api_client_ip, args.trust_proxy_header)?;
    Ok(args)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = parse_args()?;
    if let Some(warning) = loopback_socket_warning(args.listen, args.api_client_ip) {
        eprintln!("{warning}");
    }

    let store: Arc<dyn BlockStore> = match &args.database_url {
        Some(url) => {
            println!("index:  postgres");
            Arc::new(PostgresStore::connect(url, args.max_connections).await?)
        }
        None => {
            println!("index:  in-memory (no --database-url; index is lost on restart)");
            Arc::new(MemoryStore::new())
        }
    };

    let source = Arc::new(RpcSource::connect(&args.node_rpc)?);
    println!("node:   {}", args.node_rpc);

    let network = match fetch_network(&args.node_rpc).await {
        Ok(Some(network)) => network,
        Ok(None) => maya_address::Network::Devnet,
        Err(e) => {
            eprintln!("node:   chain id unavailable ({e}); explanations use devnet addresses");
            maya_address::Network::Devnet
        }
    };
    println!("text:   {} addresses", network.hrp());

    let indexer = Indexer::new(source, Arc::clone(&store)).with_network(network);
    let events = indexer.events();

    // Checked once, at startup. Mounting the route over a directory that is not
    // there would turn every icon into a 404 that looks like a browser problem;
    // saying so here makes it a deployment problem, which is what it is.
    let assets = if args.assets.is_dir() {
        println!("assets: {}", args.assets.display());
        Some(args.assets.clone())
    } else {
        eprintln!(
            "assets: {} not found - serving no icons, favicon or social card. \
             Pass --assets <DIR> pointing at the explorer's assets directory.",
            args.assets.display()
        );
        None
    };

    let state = AppState {
        api_client_ip: args.api_client_ip,
        ..AppState::new(
            Arc::clone(&store),
            events,
            Some(args.node_rpc.clone()),
            assets,
        )
    };

    let server = serve(args.listen, state).await?;
    println!("http:   http://{}", server.address);

    // The indexer runs alongside the server rather than inside a request, so a
    // browser that never connects still gets a current index.
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let indexing = tokio::spawn(indexer.run(shutdown_rx));

    println!("explorer ready");

    tokio::signal::ctrl_c().await?;
    println!("shutting down");
    let _ = shutdown_tx.send(true);
    indexing.abort();
    server.handle.abort();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> SocketAddr {
        text.parse().expect("address")
    }

    #[test]
    fn a_forgeable_header_mode_on_a_public_address_is_refused() {
        for mode in [ClientIp::CfConnectingIp, ClientIp::XForwardedForLast] {
            assert!(check_client_ip(at("0.0.0.0:3000"), mode, false).is_err());
            assert!(check_client_ip(at("[::]:3000"), mode, false).is_err());
            assert!(check_client_ip(at("127.0.0.1:3000"), mode, false).is_ok());
            assert!(check_client_ip(at("[::1]:3000"), mode, false).is_ok());
            assert!(check_client_ip(at("0.0.0.0:3000"), mode, true).is_ok());
        }
        assert!(check_client_ip(at("0.0.0.0:3000"), ClientIp::Socket, false).is_ok());
    }

    #[test]
    fn socket_mode_on_loopback_is_warned_about() {
        assert!(loopback_socket_warning(at("127.0.0.1:3000"), ClientIp::Socket).is_some());
        assert!(loopback_socket_warning(at("0.0.0.0:3000"), ClientIp::Socket).is_none());
        assert!(loopback_socket_warning(at("127.0.0.1:3000"), ClientIp::CfConnectingIp).is_none());
    }
}
