//! `maya2c fork` and `maya2c replay` (Master Prompt 24 §2–3).
//!
//! Both start the same way: a local, verified copy of another network's state,
//! taken with the node's own pruned bootstrap (`state_pruner::snapshot`) —
//! every header validated, a snapshot checked against its header's state
//! root, every block above it re-executed with its root checked. Nothing the
//! source says is trusted.
//!
//! - **replay** stops one block short of the block to replay, then executes
//!   that block locally. The node refuses a block whose state root it does not
//!   reproduce, so a successful replay *is* the proof of the exact same
//!   result; the per-account balance changes are printed.
//! - **fork** copies up to the source's tip, then serves JSON-RPC locally and
//!   makes its own blocks from its own mempool: the network's state, with a
//!   chain only this machine extends.
//!
//! Not lazy: the snapshot is downloaded whole before anything runs. Loading
//! state on demand would need a proof per read the source does not serve.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, anyhow, bail};
use custom_l1_node::consensus::{Chain, ChainConfig};
use custom_l1_node::core::{Block, BlockHeader};
use custom_l1_node::error::Result as NodeResult;
use custom_l1_node::genesis::GenesisConfig;
use custom_l1_node::network::Mempool;
use custom_l1_node::rpc::bootstrap::RpcBootstrapSource;
use custom_l1_node::rpc::{RpcContext, RpcServer};
use custom_l1_node::state::StateDB;
use custom_l1_node::state::balance_changes::BalanceChange;
use custom_l1_node::state_pruner::snapshot::{BootstrapSource, SnapshotManifest, bootstrap_pruned};

/// How often a fork makes a block from its mempool.
pub const FORK_BLOCK_INTERVAL: Duration = Duration::from_secs(1);

/// A source that pretends its tip is `cap`: how replay stops one block short.
struct Capped<'a> {
    inner: &'a dyn BootstrapSource,
    cap: u64,
}

impl BootstrapSource for Capped<'_> {
    fn tip_height(&self) -> NodeResult<u64> {
        Ok(self.inner.tip_height()?.min(self.cap))
    }
    fn headers(&self, from: u64, to: u64) -> NodeResult<Vec<BlockHeader>> {
        self.inner.headers(from, to.min(self.cap))
    }
    fn snapshot_manifest(&self) -> NodeResult<SnapshotManifest> {
        self.inner.snapshot_manifest()
    }
    fn snapshot_chunk(&self, index: usize) -> NodeResult<Vec<u8>> {
        self.inner.snapshot_chunk(index)
    }
    fn block(&self, height: u64) -> NodeResult<Block> {
        self.inner.block(height)
    }
}

/// Where to copy from.
#[derive(Clone, Debug)]
pub struct Source {
    /// The source node's JSON-RPC URL.
    pub url: String,
    /// The network's genesis file.
    pub genesis: std::path::PathBuf,
    /// An empty directory for the local copy.
    pub dir: std::path::PathBuf,
    /// `Some(depth)`: start from the source's snapshot at least `depth`
    /// below the (capped) tip. `None`: full sync — seed genesis and execute
    /// every block, which reaches any height whose body the source serves.
    pub snapshot_depth: Option<u64>,
}

fn chain_config(config: &GenesisConfig) -> anyhow::Result<ChainConfig> {
    // As the node does: DAG-BFT verifies no work, proof of work verifies
    // against the genesis floor.
    let base = if config.bft.is_some() {
        ChainConfig::without_pow_verification()
    } else {
        ChainConfig::with_pow_limit(config.pow_limit())
    };
    Ok(base.with_upgrades(&config.upgrade_schedule()?))
}

/// A verified local copy of `source`'s chain, up to `cap` if given.
/// Blocking: call it off the async runtime's worker threads.
///
/// # Errors
///
/// Anything the bootstrap refuses, or an unreadable genesis.
pub fn copy(
    source: &Source,
    cap: Option<u64>,
    runtime: tokio::runtime::Handle,
) -> anyhow::Result<(Arc<StateDB>, Chain, RpcBootstrapSource)> {
    let config = GenesisConfig::from_json(
        &std::fs::read_to_string(&source.genesis).context("reading the genesis")?,
    )?;
    if source.dir.exists() {
        std::fs::remove_dir_all(&source.dir)?;
    }
    let state = Arc::new(StateDB::open(&source.dir)?);
    let remote = RpcBootstrapSource::new(&source.url, runtime)?;
    let tip = remote.tip_height()?.min(cap.unwrap_or(u64::MAX));
    let chain = match source.snapshot_depth {
        Some(depth) => bootstrap_pruned(
            Arc::clone(&state),
            config.genesis_block()?,
            chain_config(&config)?,
            &Capped {
                inner: &remote,
                cap: tip,
            },
            depth,
        )
        .context("bootstrapping from the source's snapshot")?,
        None => full_sync(&config, &state, &remote, tip)?,
    };
    Ok((state, chain, remote))
}

/// Seeds genesis exactly as the node does, then executes every block up to
/// `tip`; `insert_block` refuses any whose state root it does not reproduce.
fn full_sync(
    config: &GenesisConfig,
    state: &Arc<StateDB>,
    remote: &RpcBootstrapSource,
    tip: u64,
) -> anyhow::Result<Chain> {
    config.seed_state(state)?;
    let mut chain = Chain::open(
        Arc::clone(state),
        config.genesis_block()?,
        chain_config(config)?,
    )?;
    for height in 1..=tip {
        let block = remote.block(height)?;
        chain
            .insert_block(block)
            .with_context(|| format!("executing block {height} locally"))?;
    }
    Ok(chain)
}

/// A replayed block.
#[derive(Clone, Debug)]
pub struct Replay {
    /// Its height.
    pub height: u64,
    /// Its id, hex.
    pub block_id: String,
    /// Its transactions' ids, hex.
    pub txids: Vec<String>,
    /// The state root it declares, which local execution reproduced.
    pub state_root: String,
    /// Every balance it moved, before and after.
    pub changes: Vec<BalanceChange>,
}

/// Replays the block at `height` locally.
///
/// # Errors
///
/// A bootstrap failure, or — the case that matters — local execution not
/// reproducing the block's state root, which the chain refuses.
pub fn replay(
    source: &Source,
    height: u64,
    runtime: tokio::runtime::Handle,
) -> anyhow::Result<Replay> {
    if height == 0 {
        bail!("genesis is not executed; there is nothing to replay");
    }
    let (state, mut chain, remote) = copy(source, Some(height - 1), runtime)?;
    if chain.height() != height - 1 {
        bail!(
            "the local copy stopped at {}, not {}",
            chain.height(),
            height - 1
        );
    }
    let block = remote.block(height)?;
    let id = block.header.id();
    let txids = block
        .transactions
        .iter()
        .map(|t| hex::encode(t.txid()))
        .collect();
    let state_root = hex::encode(block.header.state_root);
    chain
        .insert_block(block)
        .with_context(|| format!("local execution of block {height} did not reproduce it"))?;
    if chain.tip() != id {
        bail!("block {height} did not become the local tip");
    }
    let changes = state
        .balance_changes(&id)?
        .ok_or_else(|| anyhow!("no balance changes recorded"))?;
    Ok(Replay {
        height,
        block_id: hex::encode(id),
        txids,
        state_root,
        changes,
    })
}

/// A running fork: local RPC over the copied state, extended by local blocks.
pub struct Fork {
    /// The local chain.
    pub chain: Arc<Mutex<Chain>>,
    /// Its mempool.
    pub mempool: Mempool,
    /// Its RPC server.
    pub server: RpcServer,
    /// The height it was forked at.
    pub forked_at: u64,
}

/// Starts a fork of `source` at its tip, serving RPC on `rpc_addr`.
///
/// # Errors
///
/// A bootstrap failure or an address that cannot be bound.
pub async fn start(source: Source, rpc_addr: std::net::SocketAddr) -> anyhow::Result<Fork> {
    let handle = tokio::runtime::Handle::current();
    let (state, chain, _) =
        tokio::task::spawn_blocking(move || copy(&source, None, handle)).await??;
    let forked_at = chain.height();
    let chain = Arc::new(Mutex::new(chain));
    let mempool = Mempool::new(state);
    let server = custom_l1_node::rpc::serve(
        rpc_addr,
        RpcContext::new(Arc::clone(&chain), mempool.clone()),
    )
    .await?;
    tokio::spawn(produce(Arc::clone(&chain), mempool.clone()));
    Ok(Fork {
        chain,
        mempool,
        server,
        forked_at,
    })
}

/// Makes a block from the mempool every [`FORK_BLOCK_INTERVAL`], when it has
/// anything in it.
async fn produce(chain: Arc<Mutex<Chain>>, mempool: Mempool) {
    loop {
        tokio::time::sleep(FORK_BLOCK_INTERVAL).await;
        let pending = mempool.snapshot();
        if pending.is_empty() {
            continue;
        }
        let Ok(mut chain) = chain.lock() else { return };
        let timestamp = chain.get(&chain.tip()).map_or(0, |r| r.header.timestamp) + 1;
        let included: Vec<[u8; 32]> = pending
            .iter()
            .map(custom_l1_node::core::Transaction::txid)
            .collect();
        match chain
            .candidate_block(timestamp, pending)
            .and_then(|b| chain.insert_block(b))
        {
            Ok(_) => {
                mempool.remove_all(&included);
            }
            Err(error) => eprintln!("fork: could not make a block: {error}"),
        }
    }
}
