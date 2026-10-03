//! L1 node daemon.
//!
//! Loads a genesis file, opens persistent state, joins the P2P mesh, serves
//! JSON-RPC, and either runs DAG-BFT (a genesis with a `bft` committee) or
//! follows and optionally mines proof of work (a genesis without one).
//!
//! ```text
//! node --genesis /config/genesis.json \
//!      --data-dir /data \
//!      --rpc-addr 0.0.0.0:8545 \
//!      --p2p-port 30333 \
//!      --bootnode /dns4/seed-node/tcp/30333 \
//!      --mine --threads 2
//! ```
//!
//! ## Node identity
//!
//! The libp2p keypair is persisted at `<data-dir>/node_key`. A seed node that
//! regenerated its identity on restart would change its `PeerId` and invalidate
//! every bootnode address pointing at it, so the key is written once and reused.

use std::error::Error;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use custom_l1_node::config::NodeConfig;
use custom_l1_node::consensus::{
    Chain, ChainConfig, InsertOutcome, PowMode, mine_header_with, suggested_threads,
};
use custom_l1_node::core::Block;
use custom_l1_node::genesis::GenesisConfig;
use custom_l1_node::metrics::{Metrics, server as metrics_server};
use custom_l1_node::network::EpochClock;
use custom_l1_node::network::pq::dual::DualKemPolicy;
use custom_l1_node::network::{Mempool, Node, NodeHandle, load_or_create_identity};
use custom_l1_node::rpc::{
    MarketFeed, MarketQuote, MarketState, RpcContext, serve_market, serve_metered,
};
use custom_l1_node::state::StateDB;
use custom_l1_node::state_pruner::PRUNE_DEPTH;

use libp2p::Multiaddr;

// Ordinary sibling modules. They needed `#[path]` while this was
// `crates/node/src/bin/node.rs`, because a `.rs` directly in `src/bin/` is
// built as a binary of its own; in a package of its own that constraint is
// gone.
mod bft;
mod import;
mod pruning;
use import::{backfill_loop, block_import_loop};
use pruning::{open_or_bootstrap, pruning_loop, pruning_services};

/// Subdirectory holding the RocksDB state.
const STATE_DIR: &str = "state";

/// Pause between mining rounds, so a node that keeps losing races does not spin.
const MINE_BACKOFF: Duration = Duration::from_millis(250);

/// How often gauges are resampled.
///
/// Well under a typical 15-second Prometheus scrape, so a scrape never reads a
/// sample it has already seen.
const METRICS_SAMPLE_INTERVAL: Duration = Duration::from_secs(5);

/// How often the transport's rotation clock is told the local chain height.
///
/// Coarse on purpose. The rotation period is a thousand blocks — over four
/// hours — so sampling every fifteen seconds bounds the error at one block.
const ROTATION_HEIGHT_SAMPLE_INTERVAL: Duration = Duration::from_secs(15);

/// How far ahead of the tip the next epoch's cache is generated.
///
/// A hundred blocks is twenty-five minutes at a 15-second target, against a
/// generation that takes about a second. Deliberately far more headroom than
/// the job needs: the cost of being early is one extra 64 MiB allocation that
/// was going to be made anyway, and the cost of being late is every node on the
/// network stalling on the same block.
const DAG_PREPARE_LOOKAHEAD: u64 = 100;

/// Chain ids treated as carrying real value.
///
/// A node serving one of these refuses to start while the shielded pool's
/// circuit is unaudited, because a missing constraint lets anyone mint hidden
/// coins that no supply audit would reveal.
const VALUE_BEARING_CHAINS: &[&str] = &["maya-mainnet", "mainnet"];

/// The consensus mode a genesis file selects (ADR-015 §5: the mode is a
/// genesis parameter, never a per-host setting). A `bft` committee means
/// `dag-bft`; its absence means the proof-of-work devnet mode.
fn consensus_mode(config: &GenesisConfig) -> &'static str {
    if config.bft.is_some() {
        "dag-bft"
    } else {
        "argonblake-pow"
    }
}

/// Refuses a devnet-only consensus mode in a production build (Master Prompt
/// 11 §3, ADR-016): a mainnet binary must never run a devnet mode on a network
/// that expects mainnet rules.
fn check_consensus_mode(mode: &str) -> Result<(), String> {
    if cfg!(feature = "production") && mode != "dag-bft" {
        return Err(format!(
            "production build: consensus mode `{mode}` is devnet-only; mainnet runs dag-bft, \
             selected by a `bft` committee in genesis (ADR-015, ADR-027)"
        ));
    }
    Ok(())
}

struct Args {
    genesis: PathBuf,
    data_dir: PathBuf,
    rpc_addr: SocketAddr,
    metrics_addr: Option<SocketAddr>,
    market_addr: Option<SocketAddr>,
    market_feed: Option<PathBuf>,
    p2p_port: u16,
    bootnodes: Vec<Multiaddr>,
    sync_from: Option<String>,
    mine: bool,
    threads: usize,
    dual_kem: DualKemPolicy,
    config: Option<PathBuf>,
    /// Pruning depth; `None` is an archive node, which prunes nothing.
    prune_depth: Option<u64>,
    /// Prune without keeping any archive, like a Bitcoin pruned node.
    prune_without_archive: bool,
    /// Local archive directory; defaults to `<data-dir>/archive`.
    archive_dir: Option<PathBuf>,
    /// kubo RPC API to archive to and fetch from, e.g. http://127.0.0.1:5001.
    ipfs_api: Option<String>,
    /// Arweave gateway to fetch archived batches from. Read-only.
    arweave_gateway: Option<String>,
    /// Take a state snapshot every this many blocks, to serve to pruned nodes.
    snapshot_interval: Option<u64>,
    /// Bootstrap a pruned node from this peer's JSON-RPC endpoint.
    bootstrap_from: Option<String>,
    /// DAG-BFT: catch up through this peer's attested checkpoint and follow
    /// attested blocks (ADR-038), for a node that was down too long to rejoin.
    catch_up_from: Option<String>,
    /// ML-DSA-65 validator key; absent means an observer on a DAG-BFT network.
    validator_key: Option<PathBuf>,
    /// The validator key lives in `maya2c-signer` at this address instead
    /// (ADR-033). Needs `signer_pin`, `validator_pubkey` and `signer_identity`.
    remote_signer: Option<SocketAddr>,
    /// The signer's channel public key, hex: the only signer this node talks to.
    signer_pin: Option<String>,
    /// The validator public key the signer holds, hex.
    validator_pubkey: Option<String>,
    /// This node's channel identity, written by `--generate-signer-identity`.
    signer_identity: Option<PathBuf>,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            genesis: PathBuf::from("genesis.json"),
            data_dir: PathBuf::from("./data"),
            rpc_addr: "0.0.0.0:8545".parse().expect("valid default address"),
            // Off unless asked for. When enabled the deployment binds it to the
            // pod network only — the exporter publishes peer topology and
            // mempool contents, which are not public data.
            metrics_addr: None,
            // Also off unless asked for, but for the opposite reason: what it
            // serves is public, and the supply scan behind it is the most
            // expensive read the node offers. Turn it on where it is wanted.
            market_addr: None,
            market_feed: None,
            p2p_port: 30333,
            bootnodes: Vec::new(),
            sync_from: None,
            mine: false,
            threads: suggested_threads(),
            // Off. The dual-KEM transport pairs a released ML-KEM with a
            // release candidate tracking a draft standard, and the combiner
            // needs both halves to agree — so a bug in the newer one breaks
            // every connection that negotiated it. Opting in is a decision an
            // operator makes; see `docs/pq-transport.md`.
            dual_kem: DualKemPolicy::Disabled,
            // No path unless asked. An absent file means the compiled
            // defaults, which is what every existing deployment already runs.
            config: None,
            // Archive node unless asked. Pruning trades history for disk and
            // commits the node to refusing reorgs below its horizon, which is
            // an operator's decision.
            prune_depth: None,
            prune_without_archive: false,
            archive_dir: None,
            ipfs_api: None,
            arweave_gateway: None,
            snapshot_interval: None,
            bootstrap_from: None,
            catch_up_from: None,
            validator_key: None,
            remote_signer: None,
            signer_pin: None,
            validator_pubkey: None,
            signer_identity: None,
        }
    }
}

fn print_usage() {
    println!(
        "node — L1 node daemon\n\n\
         USAGE:\n  \
         node [OPTIONS]\n\n\
         OPTIONS:\n  \
         --genesis <PATH>     genesis.json path (default genesis.json)\n  \
         --data-dir <PATH>    persistent data directory (default ./data)\n  \
         --rpc-addr <ADDR>    JSON-RPC listen address (default 0.0.0.0:8545)\n  \
  --metrics-addr <ADDR> Prometheus exporter address; off unless set\n  \
  --market-addr <ADDR>  aggregator supply/ticker HTTP address; off unless set\n  \
  --market-feed <PATH>  JSON array of exchange quotes for the ticker endpoints\n  \
         --p2p-port <PORT>    libp2p TCP port (default 30333)\n  \
         --bootnode <ADDR>    peer multiaddr to dial; repeatable\n  \
         --sync-from <URL>    peer JSON-RPC endpoint to backfill history from\n  \
         --dual-kem <MODE>    HQC alongside ML-KEM: off (default), preferred,\n  \
         \x20                    or required. See docs/pq-transport.md\n  \
         --prune              prune bodies older than one DAG epoch (30,000 blocks)\n  \
         --prune-depth <N>    prune bodies older than N blocks (implies --prune)\n  \
         --prune-without-archive  prune without writing any archive first\n  \
         --archive-dir <PATH> local archive directory (default <data-dir>/archive)\n  \
         --ipfs-api <URL>     also archive to a kubo node, e.g. http://127.0.0.1:5001\n  \
         --arweave-gateway <URL>  also fetch archived batches from an Arweave gateway\n  \
         --snapshot-interval <N>  snapshot state every N blocks for pruned peers\n  \
         --bootstrap-from <URL>   bootstrap a pruned node from a peer's JSON-RPC\n  \
         --catch-up-from <URL>    DAG-BFT: rejoin after a long outage through a peer's\n                           \
         attested checkpoint, then follow attested blocks (ADR-038)\n  \
         --validator-key <PATH>  DAG-BFT validator key; without it the node observes\n  \
         --generate-validator-key <PATH>  write a new validator key, print its public key\n  \
         --remote-signer <ADDR>  sign through maya2c-signer, not a key file (ADR-033)\n  \
         --signer-pin <HEX>   the signer's channel public key\n  \
         --validator-pubkey <HEX>  the validator public key the signer holds\n  \
         --signer-identity <PATH>  this node's channel identity\n  \
         --generate-signer-identity <PATH>  write one, print the public key to pin\n  \
         --mine               mine blocks on this node (proof-of-work devnets only)\n  \
         --threads <N>        mining threads (default: available parallelism, max 8)\n  \
         -h, --help           show this message"
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
            "--genesis" => args.genesis = PathBuf::from(value()?),
            "--data-dir" => args.data_dir = PathBuf::from(value()?),
            "--rpc-addr" => args.rpc_addr = value()?.parse()?,
            "--metrics-addr" => args.metrics_addr = Some(value()?.parse()?),
            "--market-addr" => args.market_addr = Some(value()?.parse()?),
            "--market-feed" => args.market_feed = Some(PathBuf::from(value()?)),
            "--p2p-port" => args.p2p_port = value()?.parse()?,
            "--bootnode" => args.bootnodes.push(value()?.parse()?),
            "--sync-from" => args.sync_from = Some(value()?),
            "--config" => args.config = Some(PathBuf::from(value()?)),
            // Emitted from `NodeConfig::default()` rather than a string
            // constant, so a changed default cannot leave a stale template on
            // disk claiming otherwise. `deploy_bootstrap.sh` calls this.
            "--print-config-template" => {
                print!("{}", NodeConfig::template());
                std::process::exit(0);
            }
            "--dual-kem" => args.dual_kem = value()?.parse()?,
            // Embedded mining is a proof-of-work devnet tool; a production
            // build does not accept the flag at all (ADR-016).
            "--mine" if cfg!(feature = "production") => {
                return Err(
                    "--mine is a devnet flag and is not available in a production build".into(),
                );
            }
            "--mine" => args.mine = true,
            "--prune" => args.prune_depth = args.prune_depth.or(Some(PRUNE_DEPTH)),
            "--prune-depth" => args.prune_depth = Some(value()?.parse()?),
            "--prune-without-archive" => args.prune_without_archive = true,
            "--archive-dir" => args.archive_dir = Some(PathBuf::from(value()?)),
            "--ipfs-api" => args.ipfs_api = Some(value()?),
            "--arweave-gateway" => args.arweave_gateway = Some(value()?),
            "--snapshot-interval" => args.snapshot_interval = Some(value()?.parse()?),
            "--bootstrap-from" => args.bootstrap_from = Some(value()?),
            "--catch-up-from" => args.catch_up_from = Some(value()?),
            "--validator-key" => args.validator_key = Some(PathBuf::from(value()?)),
            "--remote-signer" => args.remote_signer = Some(value()?.parse()?),
            "--signer-pin" => args.signer_pin = Some(value()?),
            "--validator-pubkey" => args.validator_pubkey = Some(value()?),
            "--signer-identity" => args.signer_identity = Some(PathBuf::from(value()?)),
            "--generate-signer-identity" => {
                bft::generate_signer_identity(Path::new(&value()?))?;
                std::process::exit(0);
            }
            "--generate-validator-key" => {
                bft::generate_validator_key(Path::new(&value()?))?;
                std::process::exit(0);
            }
            "--threads" => args.threads = value()?.parse()?,
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    Ok(args)
}

/// Loads exchange quotes from a JSON file, if one was named.
///
/// No path means no feed, which is the honest state for an asset that does not
/// trade yet. A path that was given but cannot be read or parsed is fatal:
/// falling back to an empty feed would leave the ticker endpoints answering
/// `503` while the operator believes they configured them, and the only symptom
/// would be an aggregator quietly not listing the asset.
fn load_market_feed(path: Option<&Path>) -> Result<MarketFeed, Box<dyn Error>> {
    let Some(path) = path else {
        return Ok(MarketFeed::empty());
    };

    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let quotes: Vec<MarketQuote> =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;

    Ok(MarketFeed::with_quotes(quotes))
}

/// Takes the chain lock, recovering it if a previous holder panicked.
///
/// Refusing to proceed on a poisoned lock would take the node down
/// permanently; state itself is protected by RocksDB's atomic batches.
fn lock_chain(chain: &Mutex<Chain>) -> std::sync::MutexGuard<'_, Chain> {
    chain
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Seconds since the Unix epoch.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// Refreshes the gauges that describe current state.
///
/// Polled rather than pushed: peer count and mempool depth live behind locks
/// owned by other tasks, and sampling them on a timer keeps the metrics code
/// out of the hot paths that actually move blocks.
async fn metrics_sample_loop(
    chain: Arc<Mutex<Chain>>,
    network: NodeHandle,
    metrics: Arc<Metrics>,
    pq: custom_l1_node::network::SessionStats,
    epoch: EpochClock,
) {
    let mut ticker = tokio::time::interval(METRICS_SAMPLE_INTERVAL);

    loop {
        ticker.tick().await;

        if let Ok(peers) = network.connected_peers().await {
            metrics.set_peers(peers.len());
        }
        metrics.set_mempool_depth(network.mempool().len());
        metrics.set_pq_sessions(pq.established(), pq.rotated(), epoch.current());

        let (height, tip, state) = {
            let chain = lock_chain(&chain);
            (chain.height(), chain.tip(), Arc::clone(chain.state()))
        };
        metrics.set_height(height);

        // The target the *next* block must meet is the difficulty operators
        // care about, not the one the tip already satisfied.
        if let Ok(target) = lock_chain(&chain).next_target(&tip) {
            metrics.set_difficulty(&target);
        }

        if let Ok(pool) = state.stored_pool() {
            metrics.set_shielded(pool.note_count(), pool.balance());
        }

        // DAG-BFT with staking: slots each committee member led this epoch
        // whose anchor never committed (SLO validator-downtime).
        if let Ok(Some(record)) = state.committed_staking() {
            let missed: Vec<([u8; 32], u64)> = record
                .expected
                .iter()
                .map(|(id, e)| {
                    (
                        *id,
                        e.saturating_sub(record.authored.get(id).copied().unwrap_or(0)),
                    )
                })
                .collect();
            metrics.set_missed_rounds(&missed);
        }
    }
}

/// Feeds local chain height to the transport''s rotation clock.
///
/// Its own task, and always spawned, because rotation must not depend on
/// optional machinery. Putting this in `metrics_sample_loop` would have been
/// less code and would have meant that a node started without `--metrics-addr`
/// silently fell back to wall-clock rotation — the schedule would still work,
/// but it would no longer be the schedule the operator was told about.
///
/// Sampling rather than hooking import or mining: height advances by both
/// paths, and a sampler cannot miss one of them.
async fn rotation_height_loop(chain: Arc<Mutex<Chain>>, epoch: EpochClock) {
    let mut ticker = tokio::time::interval(ROTATION_HEIGHT_SAMPLE_INTERVAL);

    loop {
        ticker.tick().await;
        epoch.set_height(lock_chain(&chain).height());
    }
}

/// Refuses the peers every active on-chain threat indicator names, at the tip.
///
/// Sampled beside the rotation clock and for the same reason: height advances
/// through import and mining, and a sampler cannot miss either. The node
/// replaces its whole convicted set on each push, so only a changed set is
/// pushed, and a failed push is simply retried on the next tick. While threat
/// intel is dark no indicator exists and this pushes nothing.
async fn threat_enforcement_loop(chain: Arc<Mutex<Chain>>, network: NodeHandle) {
    let mut ticker = tokio::time::interval(ROTATION_HEIGHT_SAMPLE_INTERVAL);
    let mut enforced: Vec<[u8; 32]> = Vec::new();

    loop {
        ticker.tick().await;
        let (height, state) = {
            let chain = lock_chain(&chain);
            (chain.height(), Arc::clone(chain.state()))
        };
        let mitigations = match state.active_mitigations(height) {
            Ok(mitigations) => mitigations,
            Err(error) => {
                eprintln!("could not read threat indicators: {error}");
                continue;
            }
        };
        // Author order is the store's key order, so equal sets compare equal.
        let authors: Vec<[u8; 32]> = mitigations.iter().map(|m| m.author).collect();
        if authors == enforced {
            continue;
        }
        match network.enforce_mitigations(&mitigations).await {
            Ok(()) => enforced = authors,
            Err(error) => eprintln!("could not enforce threat mitigations: {error}"),
        }
    }
}

/// Generates the next epoch's verification cache before the chain needs it.
///
/// Without this, the first block of a new epoch pays a ~0.9 s cache generation
/// *inside* `insert_block`, under the chain lock, while the node is also trying
/// to relay. That is a stall every 5.21 days on every node at once, at the
/// moment the network is least able to absorb one.
///
/// Sampling on the same interval as the rotation clock and for the same reason:
/// height advances through both import and mining, and a sampler cannot miss
/// one of them. Generation runs on the blocking pool because it is a second of
/// CPU, and it is idempotent — the registry hands back an already-held cache
/// without rebuilding, so an early or repeated call costs nothing.
async fn dag_prepare_loop(chain: Arc<Mutex<Chain>>) {
    let mut ticker = tokio::time::interval(ROTATION_HEIGHT_SAMPLE_INTERVAL);

    loop {
        ticker.tick().await;

        let (dag, height) = {
            let chain = lock_chain(&chain);
            (Arc::clone(chain.dag()), chain.height())
        };

        // The epoch of a block far enough ahead that generation finishes first.
        let horizon = height.saturating_add(DAG_PREPARE_LOOKAHEAD);
        if !dag.is_active(horizon) {
            continue;
        }

        let epoch = dag.config().epoch_of(horizon);
        if dag.held_epochs().contains(&epoch) {
            continue;
        }

        let result = tokio::task::spawn_blocking(move || dag.prepare(epoch)).await;
        match result {
            Ok(Ok(())) => println!("generated the proof-of-work cache for epoch {epoch}"),
            Ok(Err(error)) => eprintln!("could not prepare epoch {epoch}: {error}"),
            Err(error) => eprintln!("cache preparation task failed: {error}"),
        }
    }
}

/// Mines continuously, submitting each solution to the shared chain.
async fn mining_loop(chain: Arc<Mutex<Chain>>, network: NodeHandle, threads: usize) {
    // Shared so each round's blocking task gets its own handle; a bare
    // AtomicBool would be moved into the first closure and gone by round two.
    let cancel = Arc::new(AtomicBool::new(false));

    loop {
        // Build a candidate against the current tip, releasing the lock before
        // the expensive search so RPC and block import are not blocked for the
        // duration of a mining round.
        //
        // The guard must not survive into the error path either: a std
        // MutexGuard is not Send, so holding one across an await would make
        // this whole future unspawnable. Everything the lock protects is
        // resolved into a plain value first.
        let prepared = {
            let chain = lock_chain(&chain);
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let height = chain.height() + 1;
            chain
                .candidate_header(timestamp)
                .map(|header| (header, height))
        };

        let (candidate, target_height) = match prepared {
            Ok(value) => value,
            Err(error) => {
                eprintln!("miner: cannot build a candidate: {error}");
                tokio::time::sleep(MINE_BACKOFF).await;
                continue;
            }
        };

        // Hashing is CPU-bound under either rule; keep it off the async runtime
        // entirely. Cache generation at an epoch boundary is CPU-bound too, and
        // is deliberately inside this closure for the same reason.
        let round_cancel = Arc::clone(&cancel);
        let dag = Arc::clone(lock_chain(&chain).dag());
        let search = tokio::task::spawn_blocking(move || {
            let cache = if dag.is_active(target_height) {
                Some(dag.cache_for_height(target_height)?)
            } else {
                None
            };
            let mode = cache.as_deref().map_or(PowMode::Argon, PowMode::DagLight);
            mine_header_with(&candidate, threads, &round_cancel, None, &mode)
        })
        .await;

        let solved = match search {
            Ok(Ok(Some(result))) => result,
            Ok(Ok(None)) => continue,
            Ok(Err(error)) => {
                eprintln!("miner: hashing failed: {error}");
                tokio::time::sleep(MINE_BACKOFF).await;
                continue;
            }
            Err(error) => {
                eprintln!("miner: task failed: {error}");
                tokio::time::sleep(MINE_BACKOFF).await;
                continue;
            }
        };

        // A real miner would drain the mempool here. Block assembly from pooled
        // transactions is not wired up yet, so blocks are empty.
        let block = Block::new(solved.header, Vec::new());

        let outcome = {
            let mut chain = lock_chain(&chain);
            chain.insert_block(block.clone())
        };

        match outcome {
            Ok(InsertOutcome::Extended { tip }) => {
                println!(
                    "mined height {target_height} nonce {} tip {}",
                    block.header.nonce,
                    hex::encode(&tip[..8])
                );
                if let Err(error) = network.publish_block(&block).await {
                    // A seed node that starts first has no peers subscribed to
                    // the block topic yet. That is the expected steady state
                    // until someone dials in, so it is not worth a warning on
                    // every block — it would drown the log a real fault needs.
                    let message = error.to_string();
                    if !message.contains("NoPeersSubscribedToTopic") {
                        eprintln!("miner: could not broadcast block: {error}");
                    }
                }
            }
            Ok(other) => {
                // Another node won this height while we were hashing.
                println!("mined a block that did not extend the tip: {other:?}");
            }
            Err(error) => eprintln!("miner: block rejected: {error}"),
        }

        tokio::time::sleep(MINE_BACKOFF).await;
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // Operators ask a deployed binary what it is before anything else.
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("{} {}", env!("CARGO_BIN_NAME"), env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let args = parse_args()?;

    // The file first, then the flags over it. A flag beats the file so an
    // operator can override one setting for one run without editing state that
    // outlives the run.
    //
    // An absent file is the defaults; a file that exists and is wrong is a
    // startup failure, because the operator meant something by it.
    // Named `node_config` rather than `config`: `config` already means the
    // genesis configuration further down, and the two must never be confused —
    // one is operational and local, the other is consensus and shared.
    let node_config = match &args.config {
        Some(path) => NodeConfig::from_path(path)?,
        None => NodeConfig::load_or_default(&custom_l1_node::config::default_path())?,
    };
    println!(
        "config: p2p {}, rpc {}, rate {}/s burst {}, block cache {} MiB",
        node_config.network.p2p_port,
        node_config.rpc.listen,
        node_config.rpc.rate_limit_per_second,
        node_config.rpc.rate_limit_burst,
        node_config.storage.block_cache_mib,
    );

    std::fs::create_dir_all(&args.data_dir)?;

    let genesis_json = std::fs::read_to_string(&args.genesis)
        .map_err(|e| format!("reading genesis file {}: {e}", args.genesis.display()))?;
    let config = GenesisConfig::from_json(&genesis_json)?;
    let mode = consensus_mode(&config);
    check_consensus_mode(mode)?;
    if mode == "dag-bft" && args.mine {
        return Err("--mine is a proof-of-work flag; this genesis runs dag-bft".into());
    }

    // Refuse to serve a value-bearing chain while shielded transactions rest on
    // an unaudited circuit. Failing at startup is the point: the alternative is
    // a live mainnet whose supply cannot be audited, and an inflation bug there
    // leaves no trace to notice later.
    if VALUE_BEARING_CHAINS.contains(&config.chain_id.as_str())
        && !maya_zk_stark::pool::circuit_is_audited()
    {
        return Err(custom_l1_node::NodeError::UnauditedShieldedCircuit {
            network: config.chain_id.clone(),
        }
        .into());
    }

    // The same refusal for zkML, which rests on an SRS derived from a public
    // seed. Inert while the activation height is u64::MAX; it is here so that
    // choosing a height cannot also quietly choose mainnet.
    custom_l1_node::state::zkml::check_setup(
        &config.chain_id,
        custom_l1_node::state::context::ZKML_ACTIVATION_HEIGHT,
    )?;

    // Tuned from the configuration rather than `Options::default()`, whose
    // 8 MiB block cache makes almost every account read reach the disk.
    let state = Arc::new(StateDB::open_tuned(
        args.data_dir.join(STATE_DIR),
        &node_config.storage,
    )?);

    let genesis_block = config.genesis_block()?;
    // DAG-BFT verifies no work: a block is derived from certificates by this
    // node, and nothing else may insert one (no gossip import, no
    // `submit_block`, below). Proof of work verifies against the genesis floor.
    let chain_config = if mode == "dag-bft" {
        ChainConfig::without_pow_verification()
    } else {
        ChainConfig::with_pow_limit(config.pow_limit())
    }
    .with_upgrades(&config.upgrade_schedule()?);
    let metrics = Arc::new(Metrics::new());
    let opened = std::time::Instant::now();
    let chain = Arc::new(Mutex::new(
        open_or_bootstrap(&args, &config, &state, genesis_block.clone(), chain_config).await?,
    ));
    metrics.set_sync_duration(opened.elapsed());

    // A tip already at or past an unsupported upgrade: say so at start-up
    // rather than on the first block that arrives.
    custom_l1_node::upgrade::refuse_unsupported(
        chain_config.unsupported_upgrade,
        lock_chain(&chain).height() + 1,
    )?;

    println!("chain id:    {}", config.chain_id);
    println!("consensus:   {mode}");
    println!("genesis id:  {}", hex::encode(genesis_block.header.id()));
    println!("difficulty:  {} leading zero bits", config.difficulty_bits);
    {
        let chain = lock_chain(&chain);
        println!(
            "tip:         height {} {}",
            chain.height(),
            hex::encode(chain.tip())
        );
        println!("state root:  {}", hex::encode(state.state_root()?));
        match chain.prune_horizon() {
            0 => println!("history:     complete"),
            horizon => println!("history:     pruned below height {horizon}"),
        }
    }

    let rpc_pool = Mempool::new(Arc::clone(&state));
    let rpc_context =
        RpcContext::new(Arc::clone(&chain), rpc_pool.clone()).with_network(config.chain_id.clone());
    let checkpoint_slot = Arc::new(Mutex::new(None));
    let status_slot = Arc::new(Mutex::new(custom_l1_node::rpc::types::BftStatus::default()));
    let rpc_context = if mode == "dag-bft" {
        rpc_context
            .refusing_blocks()
            .with_checkpoints(Arc::clone(&checkpoint_slot))
            .with_bft_status(Arc::clone(&status_slot))
    } else {
        rpc_context
    };
    let (rpc_context, pruning) = pruning_services(&args, &config.chain_id, rpc_context)?;

    // --- P2P ---
    let identity = load_or_create_identity(&args.data_dir)?;
    let mut p2p =
        Node::new_tcp_with_identity_and_dual_kem(Arc::clone(&state), identity, args.dual_kem)?;
    // Logged unconditionally, including the default. An operator debugging a
    // connection failure between two nodes needs to know which side offered
    // what, and "the line is absent" is a worse answer than "off".
    println!(
        "post-quantum transport: ML-KEM-768, dual-KEM {}",
        args.dual_kem
    );
    let listen: Multiaddr = format!("/ip4/0.0.0.0/tcp/{}", args.p2p_port).parse()?;
    p2p.listen_on(listen)?;
    // Taken before `spawn` consumes the node. The epoch clock is also how the
    // block import loop tells the transport how far the chain has come, which
    // is what drives session rotation off chain height rather than the clock.
    let pq_stats = p2p.session_stats();
    let pq_epoch = p2p.epoch_clock();
    let network = p2p.spawn();
    tokio::spawn(rotation_height_loop(Arc::clone(&chain), pq_epoch.clone()));
    tokio::spawn(threat_enforcement_loop(Arc::clone(&chain), network.clone()));
    tokio::spawn(dag_prepare_loop(Arc::clone(&chain)));

    println!("peer id:     {}", network.peer_id());
    println!("p2p port:    {}", args.p2p_port);

    for bootnode in &args.bootnodes {
        match network.dial(bootnode.clone()).await {
            Ok(()) => println!("dialed bootnode {bootnode}"),
            // A bootnode that is not up yet is not fatal; the mesh converges as
            // peers appear.
            Err(error) => eprintln!("could not dial {bootnode}: {error}"),
        }
    }

    // --- RPC ---
    let limiter = Arc::new(custom_l1_node::rpc::limit::RateLimiter::new(
        node_config.rpc.rate_limit_per_second,
        node_config.rpc.rate_limit_burst,
    ));
    let rpc = serve_metered(args.rpc_addr, rpc_context, Arc::clone(&metrics), limiter).await?;
    println!("rpc:         http://{}", rpc.address);

    // --- Metrics ---
    match args.metrics_addr {
        Some(address) => {
            let exporter = metrics_server::serve(address, Arc::clone(&metrics)).await?;
            println!("metrics:     http://{}/metrics", exporter.address);
            tokio::spawn(metrics_sample_loop(
                Arc::clone(&chain),
                network.clone(),
                Arc::clone(&metrics),
                pq_stats.clone(),
                pq_epoch.clone(),
            ));
        }
        None => println!("metrics:     disabled"),
    }

    // --- Market data ---
    match args.market_addr {
        Some(address) => {
            let feed = load_market_feed(args.market_feed.as_deref())?;
            if feed.is_configured() {
                println!("market feed: {} quote(s)", feed.quotes().len());
            } else {
                // Said out loud because the ticker endpoints answer 503 in this
                // state, and an operator chasing that should not have to read
                // the source to find out why.
                println!("market feed: none; ticker endpoints will answer 503");
            }

            let market = serve_market(
                address,
                Arc::new(MarketState::new(Arc::clone(&chain), feed)),
            )
            .await?;
            println!("market:      http://{}/api/v1/supply", market.address);
        }
        None => println!("market:      disabled"),
    }

    if let Some(committee) = &config.bft {
        let signer = match (&args.validator_key, args.remote_signer) {
            (Some(_), Some(_)) => {
                return Err(
                    "--validator-key and --remote-signer are exclusive: one key, one place".into(),
                );
            }
            (Some(path), None) => Some(bft::load_validator_key(path)?.into()),
            (None, Some(addr)) => Some(bft::remote_signer(
                addr,
                args.signer_identity
                    .as_deref()
                    .ok_or("--remote-signer needs --signer-identity")?,
                args.signer_pin
                    .as_deref()
                    .ok_or("--remote-signer needs --signer-pin")?,
                args.validator_pubkey
                    .as_deref()
                    .ok_or("--remote-signer needs --validator-pubkey")?,
            )?),
            (None, None) => None,
        };
        let setup = bft::setup(committee, signer)?;
        let (mut driver, opening) = bft::open(&setup, &args.data_dir, &chain)?;
        if let Some(url) = &args.catch_up_from {
            bft::catch_up(url, &chain, &mut driver).await?;
            tokio::spawn(bft::follow_loop(
                url.clone(),
                Arc::clone(&chain),
                driver.committee(),
            ));
        }
        let size = setup.committee.len();
        match driver.validator_id() {
            Some(id) => println!("dag-bft:     validator {id} of {size}"),
            None => println!("dag-bft:     observer of {size} validators"),
        }
        tokio::spawn(bft::bft_loop(
            Arc::clone(&chain),
            network.clone(),
            rpc_pool,
            driver,
            opening,
            size,
            Arc::clone(&metrics),
            checkpoint_slot,
            status_slot,
        ));
    } else {
        // Apply blocks arriving over gossip. Without this task the node decodes
        // every gossiped block and then drops it on the floor. Proof of work
        // only: on DAG-BFT a gossiped block is a claim nobody derived.
        tokio::spawn(block_import_loop(
            Arc::clone(&chain),
            network.clone(),
            Arc::clone(&metrics),
        ));

        // Gossip only carries blocks minted after this node joined, so a peer
        // starting from genesis needs an explicit pull to cover the gap.
        if let Some(url) = args.sync_from.clone() {
            tokio::spawn(backfill_loop(Arc::clone(&chain), url, Arc::clone(&metrics)));
        }
    }

    if let Some(pruning) = pruning {
        tokio::spawn(pruning_loop(Arc::clone(&chain), pruning));
    }

    if args.mine {
        println!("mining:      enabled ({} threads)", args.threads);
        tokio::spawn(mining_loop(
            Arc::clone(&chain),
            network.clone(),
            args.threads,
        ));
    } else {
        println!("mining:      disabled");
    }

    println!("node ready");

    // Run until interrupted. Ctrl-C is the container stop signal.
    tokio::signal::ctrl_c().await?;
    println!("shutting down");
    Ok(())
}
