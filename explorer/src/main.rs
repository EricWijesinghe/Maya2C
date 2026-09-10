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
use maya_explorer::server::{AppState, serve};
use maya_explorer::store::BlockStore;
use maya_explorer::store::memory::MemoryStore;
use maya_explorer::store::postgres::PostgresStore;

struct Args {
    node_rpc: String,
    listen: SocketAddr,
    database_url: Option<String>,
    max_connections: u32,
    /// Directory holding the branding assets.
    ///
    /// A flag rather than a compiled-in path, because this resolves against the
    /// process's working directory. The explorer is a standalone binary — the
    /// root `Dockerfile` packages only `node`, `genesis` and `peerid` — so a
    /// hardcoded relative path would serve silent 404s for every icon the
    /// moment somebody started it from anywhere but the repository root, with
    /// nothing in the logs to say why.
    assets: PathBuf,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            node_rpc: "http://127.0.0.1:8545".to_string(),
            listen: "0.0.0.0:3000".parse().expect("valid default address"),
            database_url: std::env::var("DATABASE_URL").ok(),
            max_connections: 5,
            assets: PathBuf::from("assets"),
        }
    }
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
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    Ok(args)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = parse_args()?;

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

    let indexer = Indexer::new(source, Arc::clone(&store));
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
        store: Arc::clone(&store),
        events,
        node_rpc: Some(args.node_rpc.clone()),
        assets,
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
