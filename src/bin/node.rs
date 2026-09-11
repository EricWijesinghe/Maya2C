//! L1 node daemon.
//!
//! Loads a genesis file, opens persistent state, joins the P2P mesh, serves
//! JSON-RPC, and optionally mines.
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

use std::collections::HashMap;
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
use custom_l1_node::network::{Mempool, Node, NodeEvent, NodeHandle, load_or_create_identity};
use custom_l1_node::rpc::{
    BlockInfo, MarketFeed, MarketQuote, MarketState, RpcContext, serve, serve_market,
};
use custom_l1_node::state::StateDB;

use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::HttpClientBuilder;
use jsonrpsee::rpc_params;
use libp2p::Multiaddr;
use tokio::sync::broadcast;

/// Subdirectory holding the RocksDB state.
const STATE_DIR: &str = "state";

/// Pause between mining rounds, so a node that keeps losing races does not spin.
const MINE_BACKOFF: Duration = Duration::from_millis(250);

/// Pause when backfill has nothing to fetch.
const BACKFILL_IDLE: Duration = Duration::from_secs(2);

/// Maximum buffered blocks whose parent has not arrived.
///
/// Bounded because a peer can send unlimited unattached blocks; an unbounded
/// buffer would be a memory-exhaustion vector.
const MAX_ORPHANS: usize = 512;

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
/// Groth16 parameters come from a reproducible setup, because anyone able to
/// re-run that setup can mint hidden coins that no supply audit would reveal.
const VALUE_BEARING_CHAINS: &[&str] = &["maya-mainnet", "mainnet"];

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
         --mine               mine blocks on this node\n  \
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
            "--mine" => args.mine = true,
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

/// Imports a block, buffering it if its parent has not arrived yet.
///
/// Gossip does not guarantee ordering, so a child can arrive before its parent.
/// Dropping it would strand the node until the next block it happens to receive
/// in order, so unattached blocks are held and retried once their parent lands.
fn import_block(
    chain: &Mutex<Chain>,
    block: Block,
    orphans: &mut HashMap<[u8; 32], Vec<Block>>,
    metrics: &Metrics,
) {
    let mut pending = vec![block];

    while let Some(block) = pending.pop() {
        let id = block.header.id();
        let parent = block.header.prev_hash;
        let header_timestamp = block.header.timestamp;

        let known_parent = {
            let chain = lock_chain(chain);
            chain.contains(&parent)
        };

        if !known_parent {
            // Cap the buffer: an unbounded orphan pool is a memory-exhaustion
            // vector for any peer willing to send junk.
            if orphans.len() < MAX_ORPHANS {
                orphans.entry(parent).or_default().push(block);
            } else {
                metrics.record_rejection("orphan_buffer_full");
            }
            continue;
        }

        // Both clocks are read as close to the work as possible: the age
        // measurement spans two machines and so includes their skew, while the
        // import measurement is local and does not.
        metrics.observe_block_age(header_timestamp, unix_now());
        let started = std::time::Instant::now();

        let outcome = {
            let mut chain = lock_chain(chain);
            chain.insert_block(block)
        };
        metrics.observe_import(started.elapsed());

        match outcome {
            Ok(InsertOutcome::Extended { .. }) | Ok(InsertOutcome::Reorganized { .. }) => {
                let height = lock_chain(chain).height();
                metrics.record_import();
                println!("imported block, height now {height}");
                // This block may be the parent something buffered was waiting on.
                if let Some(children) = orphans.remove(&id) {
                    pending.extend(children);
                }
            }
            Ok(_) => {
                if let Some(children) = orphans.remove(&id) {
                    pending.extend(children);
                }
            }
            Err(error) => {
                // The label is a fixed string, never the error text: a label
                // drawn from an attacker-influenced message would let one peer
                // create unbounded time series in every scraper on the network.
                metrics.record_rejection("invalid");
                eprintln!("rejected an imported block: {error}");
            }
        }
    }
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

/// Applies blocks received over gossip to the local chain.
///
/// Without this the node decodes every gossiped block and then discards it: the
/// P2P layer publishes `BlockReceived` to a broadcast channel that otherwise has
/// no subscriber, so a non-mining peer would sit at genesis forever.
async fn block_import_loop(chain: Arc<Mutex<Chain>>, network: NodeHandle, metrics: Arc<Metrics>) {
    let mut events = network.subscribe();
    let mut orphans: HashMap<[u8; 32], Vec<Block>> = HashMap::new();

    loop {
        match events.recv().await {
            Ok(NodeEvent::BlockReceived(block)) => {
                import_block(&chain, *block, &mut orphans, &metrics);
            }
            Ok(_) => {}
            Err(broadcast::error::RecvError::Lagged(missed)) => {
                // Blocks were dropped while this task was behind; backfill will
                // recover them, so this is a warning rather than a failure.
                eprintln!("block import lagged, missed {missed} event(s)");
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// Backfills missing history from a peer's JSON-RPC endpoint.
///
/// Gossip only carries blocks produced *after* a node joins, so a peer starting
/// at genesis against a chain already at height N can never catch up from gossip
/// alone. A dedicated block-sync wire protocol would be the proper fix; pulling
/// sequentially over the existing RPC is a smaller mechanism that gets a testnet
/// converged, at the cost of trusting one endpoint for history.
///
/// Blocks are still validated by [`Chain::insert_block`] on the way in, so a
/// dishonest source cannot inject an invalid block — only withhold data.
async fn backfill_loop(chain: Arc<Mutex<Chain>>, url: String, metrics: Arc<Metrics>) {
    let client = match HttpClientBuilder::default().build(&url) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("backfill disabled, bad --sync-from URL {url}: {error}");
            return;
        }
    };

    println!("backfill:    pulling history from {url}");
    let mut orphans: HashMap<[u8; 32], Vec<Block>> = HashMap::new();

    loop {
        let next_height = lock_chain(&chain).height() + 1;

        let response: Result<BlockInfo, _> = client
            .request("get_block_by_height", rpc_params![next_height])
            .await;

        match response {
            Ok(info) => match hex::decode(&info.raw)
                .ok()
                .and_then(|bytes| Block::from_bytes(&bytes).ok())
            {
                Some(block) => {
                    let before = lock_chain(&chain).height();
                    import_block(&chain, block, &mut orphans, &metrics);
                    let after = lock_chain(&chain).height();
                    if after == before {
                        // Made no progress; avoid hammering the peer.
                        tokio::time::sleep(BACKFILL_IDLE).await;
                    }
                }
                None => {
                    eprintln!("backfill: peer returned an undecodable block at {next_height}");
                    tokio::time::sleep(BACKFILL_IDLE).await;
                }
            },
            // Height not available yet: we are caught up. Poll periodically so
            // the node keeps following even if gossip drops.
            Err(_) => tokio::time::sleep(BACKFILL_IDLE).await,
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

    // Refuse to serve a value-bearing chain while shielded transactions rest on
    // a setup anyone can reproduce. Failing at startup is the point: the
    // alternative is a live mainnet whose supply cannot be audited, and an
    // inflation bug there leaves no trace to notice later.
    if VALUE_BEARING_CHAINS.contains(&config.chain_id.as_str())
        && !maya_zk_privacy::prove::setup_is_trusted()
    {
        return Err(custom_l1_node::NodeError::UntrustedShieldedSetup {
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

    // Seeding verifies the resulting state root against the config, so a node
    // started with a mismatched genesis fails here rather than silently forking.
    let state_root = config.seed_state(&state)?;
    let genesis_block = config.genesis_block()?;

    println!("chain id:    {}", config.chain_id);
    println!("genesis id:  {}", hex::encode(genesis_block.header.id()));
    println!("state root:  {}", hex::encode(state_root));
    println!("difficulty:  {} leading zero bits", config.difficulty_bits);

    let chain = Arc::new(Mutex::new(Chain::new(
        Arc::clone(&state),
        genesis_block,
        ChainConfig::with_pow_limit(config.pow_limit()),
    )));

    let mempool = Mempool::new(Arc::clone(&state));

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
    let rpc = serve(args.rpc_addr, RpcContext::new(Arc::clone(&chain), mempool)).await?;
    println!("rpc:         http://{}", rpc.address);

    // --- Metrics ---
    let metrics = Arc::new(Metrics::new());
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

    // Apply blocks arriving over gossip. Without this task the node decodes
    // every gossiped block and then drops it on the floor.
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
