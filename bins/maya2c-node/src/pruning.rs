//! The node's pruning side: opening or bootstrapping the chain, the archive
//! and snapshot services the flags ask for, and the loop that runs them.
//!
//! Split from `node.rs` to keep that file within the project's size limit. See
//! `docs/pruning.md` for what pruning is and what it commits a node to.

use std::error::Error;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use custom_l1_node::consensus::{Chain, ChainConfig};
use custom_l1_node::core::Block;
use custom_l1_node::genesis::GenesisConfig;
use custom_l1_node::rpc::RpcContext;
use custom_l1_node::rpc::bootstrap::{RpcBootstrapSource, SnapshotService};
use custom_l1_node::state::StateDB;
use custom_l1_node::state_pruner::archive::{Archiver, prune_round};
use custom_l1_node::state_pruner::cold::ColdBlocks;
use custom_l1_node::state_pruner::snapshot::{Snapshots, bootstrap_pruned};
use custom_l1_node::state_pruner::{ArchivePolicy, PRUNE_DEPTH, PruneConfig};
use maya_archive::arweave::ArweaveGateway;
use maya_archive::kubo::KuboStore;
use maya_archive::{ArchiveStore, LocalDirStore};

use super::{Args, lock_chain};

/// How often the pruning loop looks for work: a snapshot to take, a batch deep
/// enough to archive and prune.
const PRUNE_POLL: Duration = Duration::from_secs(60);

/// Snapshots kept on disk. Two, so the one being served is never the one
/// being replaced.
const SNAPSHOTS_KEPT: usize = 2;

/// Opens the stored chain, or bootstraps a pruned node into an empty database.
///
/// Genesis allocations are seeded only into a **fresh** database. Seeding an
/// evolved one wrote the genesis balances back over it, which is why a node
/// that had applied a single block could not restart.
pub(super) async fn open_or_bootstrap(
    args: &Args,
    config: &GenesisConfig,
    state: &Arc<StateDB>,
    genesis: Block,
    chain_config: ChainConfig,
) -> Result<Chain, Box<dyn Error>> {
    let fresh = state.chain_meta()?.is_none();
    if fresh && let Some(url) = &args.bootstrap_from {
        let depth = args.prune_depth.unwrap_or(PRUNE_DEPTH);
        println!("bootstrap:   pruned, from {url}, snapshot at least {depth} blocks deep");
        let source = RpcBootstrapSource::new(url, tokio::runtime::Handle::current())?;
        let state = Arc::clone(state);
        // Blocking: the source waits on the runtime for every call, which is
        // only allowed off the runtime's own threads.
        let chain = tokio::task::spawn_blocking(move || {
            bootstrap_pruned(state, genesis, chain_config, &source, depth)
        })
        .await??;
        return Ok(chain);
    }
    if fresh {
        // Verifies the resulting root against the config, so a mismatched
        // genesis fails here rather than silently forking.
        config.seed_state(state)?;
    } else if args.bootstrap_from.is_some() {
        println!("bootstrap:   ignored, the database already holds a chain");
    }
    Ok(Chain::open(Arc::clone(state), genesis, chain_config)?)
}

/// Snapshots to take for pruned peers, and pruning to do.
pub(super) struct Pruning {
    /// The snapshot service and its interval in blocks.
    snapshots: Option<(Arc<SnapshotService>, u64)>,
    /// The pruning policy, and the archiver it requires (if any).
    prune: Option<(PruneConfig, Option<Archiver>)>,
}

impl Pruning {
    /// One pass: a snapshot if one is due, then every batch deep enough.
    fn run_once(&self, chain: &Mutex<Chain>) -> Result<(), custom_l1_node::NodeError> {
        if let Some((service, interval)) = &self.snapshots {
            let chain = lock_chain(chain);
            let height = chain.height();
            let latest = service.snapshots().heights()?.last().copied().unwrap_or(0);
            if height >= latest.saturating_add(*interval) {
                // Under the chain lock, so the checkpoint and the block it is
                // labelled with cannot disagree.
                service
                    .snapshots()
                    .take(chain.state(), height, &chain.tip())?;
                service.snapshots().retain_newest(SNAPSHOTS_KEPT)?;
                println!("snapshot:    taken at height {height}");
            }
        }
        if let Some((config, archiver)) = &self.prune {
            while let Some(range) = prune_round(chain, config, archiver.as_ref())? {
                println!("pruned:      heights {}..={}", range.start(), range.end());
            }
        }
        Ok(())
    }
}

/// The stores archives are written to: the local directory always, and the
/// IPFS node if one is configured.
fn write_stores(
    args: &Args,
    archive_dir: &Path,
) -> Result<Vec<Box<dyn ArchiveStore>>, Box<dyn Error>> {
    let mut stores: Vec<Box<dyn ArchiveStore>> = vec![Box::new(LocalDirStore::new(archive_dir)?)];
    if let Some(api) = &args.ipfs_api {
        stores.push(Box::new(KuboStore::new(api.clone())?));
    }
    Ok(stores)
}

/// Snapshot interval for a validator that names none (ADR-043): with only
/// the seed serving snapshots, the seed itself could not come back after 13
/// epochs down, so every validator serves them unless told not to.
const VALIDATOR_SNAPSHOT_INTERVAL: u64 = 3600;

/// `--snapshot-interval`, else [`VALIDATOR_SNAPSHOT_INTERVAL`] for a node that
/// signs; `0` turns snapshots off.
fn snapshot_interval(args: &Args) -> Option<u64> {
    let signs = args.validator_key.is_some() || args.remote_signer.is_some();
    args.snapshot_interval
        .or_else(|| signs.then_some(VALIDATOR_SNAPSHOT_INTERVAL))
        .filter(|&interval| interval > 0)
}

/// Builds the snapshot service, the cold-block fetcher and the pruning policy
/// the flags ask for, and hangs the first two on the RPC context.
pub(super) fn pruning_services(
    args: &Args,
    chain_id: &str,
    context: RpcContext,
) -> Result<(RpcContext, Option<Pruning>), Box<dyn Error>> {
    let mut context = context;
    let depth = args.prune_depth.unwrap_or(PRUNE_DEPTH);
    // How deep a snapshot must be before it is served. Separate from
    // pruning: an archive seed (which must not prune) can still serve
    // shallow snapshots on a DAG-BFT chain, where a block is final at once.
    let snapshot_depth = args.snapshot_depth.unwrap_or(depth);
    let archive_dir = args
        .archive_dir
        .clone()
        .unwrap_or_else(|| args.data_dir.join("archive"));

    let snapshots = match snapshot_interval(args) {
        Some(interval) => {
            let service = Arc::new(SnapshotService::new(
                Snapshots::new(args.data_dir.join("snapshots"))?,
                snapshot_depth,
            ));
            context = context.with_snapshots(Arc::clone(&service));
            println!("snapshots:   every {interval} blocks, served once {snapshot_depth} deep");
            Some((service, interval))
        }
        None => None,
    };

    // Reading back what was archived, from wherever a copy may be.
    let mut read_stores = write_stores(args, &archive_dir)?;
    if let Some(gateway) = &args.arweave_gateway {
        read_stores.push(Box::new(ArweaveGateway::new(gateway.clone())?));
    }
    context = context.with_cold_blocks(Arc::new(ColdBlocks::new(read_stores)));

    let prune = match args.prune_depth {
        Some(depth) => {
            let archive = if args.prune_without_archive {
                ArchivePolicy::None
            } else {
                ArchivePolicy::Required
            };
            let config = PruneConfig {
                depth,
                archive,
                ..PruneConfig::default()
            };
            let archiver = (archive == ArchivePolicy::Required)
                .then(|| write_stores(args, &archive_dir).map(|s| Archiver::new(chain_id, s)))
                .transpose()?;
            println!(
                "pruning:     below {depth} blocks, archive {}",
                if archiver.is_some() {
                    archive_dir.display().to_string()
                } else {
                    "none".into()
                }
            );
            Some((config, archiver))
        }
        None => {
            println!("pruning:     off (archive node)");
            None
        }
    };

    let pruning = (snapshots.is_some() || prune.is_some()).then_some(Pruning { snapshots, prune });
    Ok((context, pruning))
}

/// Takes snapshots and prunes, off the runtime's threads: a checkpoint and an
/// archive upload are blocking work.
pub(super) async fn pruning_loop(chain: Arc<Mutex<Chain>>, pruning: Pruning) {
    let pruning = Arc::new(pruning);
    loop {
        tokio::time::sleep(PRUNE_POLL).await;
        let (chain, pruning) = (Arc::clone(&chain), Arc::clone(&pruning));
        match tokio::task::spawn_blocking(move || pruning.run_once(&chain)).await {
            Ok(Ok(())) => {}
            // Nothing was pruned: an archive that failed is retried next pass.
            Ok(Err(error)) => eprintln!("pruning: {error}"),
            Err(error) => eprintln!("pruning task failed: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{VALIDATOR_SNAPSHOT_INTERVAL, snapshot_interval};
    use crate::Args;

    #[test]
    fn a_validator_serves_snapshots_unless_told_not_to() {
        let validator = Args {
            validator_key: Some("validator.key".into()),
            ..Args::default()
        };
        assert_eq!(
            snapshot_interval(&validator),
            Some(VALIDATOR_SNAPSHOT_INTERVAL)
        );

        let off = Args {
            snapshot_interval: Some(0),
            ..validator
        };
        assert_eq!(snapshot_interval(&off), None);
    }

    #[test]
    fn an_observer_takes_snapshots_only_when_asked() {
        assert_eq!(snapshot_interval(&Args::default()), None);
        let asked = Args {
            snapshot_interval: Some(100),
            ..Args::default()
        };
        assert_eq!(snapshot_interval(&asked), Some(100));
    }
}
