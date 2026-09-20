//! `pool` — the Maya2C mining pool daemon.
//!
//! ```text
//! pool --node-rpc http://127.0.0.1:8545 \
//!      --treasury treasury.key \
//!      --reward-per-block 1000000000 \
//!      --fee-address <HEX>
//! ```
//!
//! ## Three listeners, three audiences
//!
//! The Stratum port is for miners and speaks the post-quantum transport. The
//! dashboard is for miners and operators and speaks HTTP. The exporter is for
//! Prometheus and is off unless asked for, because what it serves is
//! operational data that belongs inside the pod network — the same reasoning
//! `bins/maya2c-node/src/main.rs` applies to the node's own exporter.
//!
//! ## The password is not a flag
//!
//! `MAYA_POOL_TREASURY_PASSWORD`, matching the wallet. A password on a command
//! line lands in shell history and is visible in the process list; an
//! environment variable is neither by default. See [`treasury::PASSWORD_ENV`].
//!
//! ## It refuses to start rather than pay wrongly
//!
//! `--reward-per-block` has no default. This chain mints no block reward
//! (`docs/stratum-v2.md` §5), so what a found block distributes is operator
//! policy funded from a treasury account, and a plausible-looking default would
//! be a policy invented by the binary rather than chosen by the operator.

use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;

use custom_l1_node::crypto::dag::registry::{CacheRegistry, DagConfig};
use custom_l1_node::metrics::server as metrics_server;
use custom_l1_node::state::Address;
use maya_pool_service::config::PoolConfig;
use maya_pool_service::daemon;
use maya_pool_service::ledger::rocks::RocksLedger;
use maya_pool_service::metrics::PoolMetrics;
use maya_pool_service::node::NodeClient;
use maya_pool_service::payout::PayoutEngine;
use maya_pool_service::state::PoolState;
use maya_pool_service::treasury::{self, Treasury};
use maya_pool_service::validator::{DEFAULT_WORKERS, Validator};
use maya_pool_service::{server, validator};
use tokio::sync::{broadcast, mpsc};

/// Subdirectory holding the share ledger.
const LEDGER_DIR: &str = "ledger";

/// Solved blocks buffered between the validator and the submitter.
///
/// Small on purpose. If this ever fills, blocks are being found faster than the
/// node can accept them, and that is a condition to see rather than to buffer.
const BLOCK_QUEUE: usize = 16;

/// Job pushes buffered per connection before it is considered lagged.
const JOB_BUFFER: usize = 16;

/// Parsed command line.
struct Args {
    /// Pool policy and addresses.
    config: PoolConfig,
    /// Concurrent share verifications.
    validators: usize,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            config: PoolConfig::default(),
            validators: DEFAULT_WORKERS,
        }
    }
}

fn print_usage() {
    println!(
        "pool — Maya2C mining pool daemon\n\n\
         USAGE:\n  \
         pool [OPTIONS]\n\n\
         REQUIRED:\n  \
         --reward-per-block <N>  value a found block distributes, in base units.\n  \
         \x20                       This chain mints no block reward, so this is\n  \
         \x20                       operator policy paid from the treasury.\n\n\
         OPTIONS:\n  \
         --stratum-addr <ADDR>   mining listen address (default 0.0.0.0:3333)\n  \
         --api-addr <ADDR>       dashboard and JSON API address (default 0.0.0.0:8080)\n  \
         --metrics-addr <ADDR>   Prometheus exporter address; off unless set\n  \
         --node-rpc <URL>        node JSON-RPC endpoint (default http://127.0.0.1:8545)\n  \
         --data-dir <PATH>       share ledger directory (default ./pool-data)\n  \
         --treasury <PATH>       encrypted treasury keystore (default treasury.key)\n  \
         --fee-rate <F>          operator fee, 0..1 (default 0.01)\n  \
         --fee-address <HEX>     operator fee destination\n  \
         --pplns-factor <F>      window as a multiple of difficulty (default 2.0)\n  \
         --confirmations <N>     depth before credits are payable (default 60)\n  \
         --min-payout <N>        smallest payout, in base units (default 1000)\n  \
         --max-batch-value <N>   ceiling on one payout batch\n  \
         --validators <N>        concurrent share verifications (default 8)\n  \
         -h, --help              show this message\n\n\
         ENVIRONMENT:\n  \
         {} unlocks the treasury keystore. There is deliberately\n  \
         \x20                       no password flag.",
        treasury::PASSWORD_ENV
    );
}

/// Parses a 32-byte hex address.
fn parse_address(value: &str) -> Result<Address, Box<dyn Error>> {
    let bytes = hex::decode(value)?;
    bytes
        .try_into()
        .map_err(|_| "an address is 32 bytes of hex".into())
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
            "--stratum-addr" => args.config.stratum_addr = value()?.parse()?,
            "--api-addr" => args.config.api_addr = value()?.parse()?,
            "--metrics-addr" => args.config.metrics_addr = Some(value()?.parse()?),
            "--node-rpc" => args.config.node_rpc = value()?,
            "--data-dir" => args.config.data_dir = PathBuf::from(value()?),
            "--treasury" => args.config.treasury_keystore = PathBuf::from(value()?),
            "--reward-per-block" => args.config.reward_per_block = value()?.parse()?,
            "--fee-rate" => args.config.fee_rate = value()?.parse()?,
            "--fee-address" => args.config.fee_address = parse_address(&value()?)?,
            "--pplns-factor" => args.config.pplns_factor = value()?.parse()?,
            "--confirmations" => args.config.confirmations = value()?.parse()?,
            "--min-payout" => args.config.min_payout = value()?.parse()?,
            "--max-batch-value" => args.config.max_batch_value = value()?.parse()?,
            "--validators" => args.validators = value()?.parse()?,
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    if args.validators == 0 {
        return Err("--validators must be at least 1".into());
    }

    // Before anything binds, and before the keystore is even opened: a pool
    // that starts with an incoherent payout policy pays wrong amounts, and a
    // pool that refuses to start pays none.
    args.config.validate()?;
    Ok(args)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = parse_args()?;
    let config = args.config.clone();

    println!(
        "pool: stratum {} | dashboard {} | node {}",
        config.stratum_addr, config.api_addr, config.node_rpc
    );

    let ledger = Arc::new(RocksLedger::open(config.data_dir.join(LEDGER_DIR))?);
    let metrics = Arc::new(PoolMetrics::new());
    // Not configurable, and it must not become so. `ChainConfig` fixes the
    // node's DAG parameters at `DagConfig::MAINNET` and exposes no flag to
    // change them (`src/consensus/chain.rs:62`), so any other value here would
    // make the pool verify shares under a rule the chain does not use: it would
    // credit work the network rejects, and every block it submitted would be
    // refused — after the shares behind them had been paid for.
    //
    // `DagConfig::TESTING` exists for tests, which build a `PoolState` directly
    // and control both halves.
    let dag = Arc::new(CacheRegistry::new(DagConfig::MAINNET));

    let treasury = Treasury::unlock(&config.treasury_keystore)?;
    let treasury_address = treasury.address();
    println!("pool: treasury {}", hex::encode(treasury_address));

    let node = Arc::new(NodeClient::connect(&config.node_rpc)?);
    let state = Arc::new(PoolState::new(
        config.clone(),
        ledger.clone(),
        Arc::clone(&metrics),
        dag,
    ));

    let engine = Arc::new(PayoutEngine::new(
        ledger,
        Arc::clone(&node) as Arc<dyn maya_pool_service::payout::ChainView>,
        treasury,
        config.clone(),
    ));

    let (blocks, found) = mpsc::channel(BLOCK_QUEUE);
    let (jobs, _) = broadcast::channel(JOB_BUFFER);

    let validator = Validator::spawn(Arc::clone(&state), args.validators, blocks);
    println!(
        "pool: {} concurrent verifications, queue depth {}",
        args.validators,
        validator::QUEUE_CAPACITY
    );

    // Work first. Channels cannot open before there is a template to hand out,
    // so a pool that started its listener first would refuse every connection
    // for the first half-second and look broken.
    tokio::spawn(daemon::job_loop(
        Arc::clone(&state),
        Arc::clone(&node),
        jobs.clone(),
    ));
    tokio::spawn(daemon::block_loop(
        Arc::clone(&state),
        Arc::clone(&node),
        Arc::clone(&engine),
        found,
    ));
    tokio::spawn(daemon::settle_loop(Arc::clone(&state), engine));
    tokio::spawn(daemon::maintenance_loop(
        Arc::clone(&state),
        Arc::clone(&node),
        treasury_address,
    ));

    let stratum =
        daemon::serve_stratum(config.stratum_addr, Arc::clone(&state), validator, jobs).await?;
    println!("pool: stratum listening on {}", stratum.address);

    let dashboard = server::serve(config.api_addr, Arc::clone(&state)).await?;
    println!("pool: dashboard listening on {}", dashboard.address);

    if let Some(address) = config.metrics_addr {
        let exporter = metrics_server::serve(address, metrics).await?;
        println!("pool: metrics listening on {}", exporter.address);
    }

    // Run until interrupted. Ctrl-C is the container stop signal.
    tokio::signal::ctrl_c().await?;
    println!("pool: shutting down");
    Ok(())
}
