//! Getting blocks into the chain: from gossip, and from a peer's history.
//!
//! Split from `node.rs` to keep that file within the project's size limit.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use custom_l1_node::consensus::{Chain, InsertOutcome};
use custom_l1_node::core::Block;
use custom_l1_node::metrics::Metrics;
use custom_l1_node::network::peer_health::classify_import;
use custom_l1_node::network::{NodeEvent, NodeHandle, Offence};
use custom_l1_node::rpc::BlockInfo;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::HttpClientBuilder;
use jsonrpsee::rpc_params;
use libp2p::PeerId;
use tokio::sync::{broadcast, mpsc};

use super::{lock_chain, unix_now};

/// Pause when backfill has nothing to fetch.
const BACKFILL_IDLE: Duration = Duration::from_secs(2);

/// Maximum buffered blocks whose parent has not arrived.
///
/// Bounded because a peer can send unlimited unattached blocks; an unbounded
/// buffer would be a memory-exhaustion vector.
const MAX_ORPHANS: usize = 512;

/// Parents fetched in a row for one orphan before giving up. A peer that keeps
/// answering with yet another unknown parent is either very far ahead or
/// leading the node down a chain nobody else has.
const MAX_SYNC_DEPTH: usize = 64;

/// Buffered blocks waiting on a parent, keyed by that parent, with the peer to
/// blame if each proves invalid: a gossiped block's author, a fetched block's
/// responder — never a relay (see `NodeEvent::BlockReceived`).
pub(super) type Orphans = HashMap<[u8; 32], Vec<(Block, Option<PeerId>)>>;

/// What an import found that the network layer must hear about.
#[derive(Default)]
pub(super) struct ImportReport {
    /// Refused blocks, each against the peer to blame for it.
    pub(super) offences: Vec<(PeerId, Offence)>,
    /// The imported block's parent, if it is unknown and the block was buffered.
    pub(super) missing_parent: Option<[u8; 32]>,
}

/// Imports a block, buffering it if its parent has not arrived yet.
///
/// Gossip does not guarantee ordering, so a child can arrive before its parent.
/// Dropping it would strand the node until the next block it happens to receive
/// in order, so unattached blocks are held and retried once their parent lands.
pub(super) fn import_block(
    chain: &Mutex<Chain>,
    block: Block,
    source: Option<PeerId>,
    orphans: &mut Orphans,
    metrics: &Metrics,
) -> ImportReport {
    let mut report = ImportReport::default();
    let first = block.header.id();
    let mut pending = vec![(block, source)];

    while let Some((block, source)) = pending.pop() {
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
                if id == first {
                    report.missing_parent = Some(parent);
                }
                orphans.entry(parent).or_default().push((block, source));
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
                if let (Some(peer), Some(offence)) = (source, classify_import(&error)) {
                    report.offences.push((peer, offence));
                }
            }
        }
    }
    report
}

/// Hands every offence an import found to the network layer.
async fn report_offences(network: &NodeHandle, offences: Vec<(PeerId, Offence)>) {
    for (peer, offence) in offences {
        if let Err(error) = network.report_offence(peer, offence).await {
            eprintln!("could not report an offence by {peer}: {error}");
        }
    }
}

/// Ancestor walks running at once, across all relays.
const MAX_CONCURRENT_WALKS: usize = 4;

/// Fetched blocks queued for the import loop. A walk waits when it is full.
const FETCH_QUEUE: usize = 64;

/// What an ancestor walk hands back to the import loop.
enum Fetched {
    /// A block the relay served, to import as its responsibility.
    Block { source: PeerId, block: Block },
    /// The walk from this relay has ended.
    Done(PeerId),
}

/// Fetches missing ancestors from the relay that sent an orphan, one parent at
/// a time, streaming each block back to the import loop.
///
/// Its own task, not a call inside the import loop. Inline, a peer that
/// answered each request just under the timeout with yet another unknown
/// parent could stall every import for minutes, unscored — an unknown parent is
/// not an offence. Here a walk stalls only itself. What it fetches goes through
/// [`import_block`] and the count-capped orphan buffer like gossip does, so
/// nothing accumulates in the walk, and every block still meets
/// [`Chain::insert_block`] before it counts. A walk ends at [`MAX_SYNC_DEPTH`],
/// at the first parent the chain already holds, or when the relay runs out or
/// lies.
async fn walk_ancestors(
    chain: Arc<Mutex<Chain>>,
    network: NodeHandle,
    source: PeerId,
    mut wanted: [u8; 32],
    fetched: mpsc::Sender<Fetched>,
) {
    for _ in 0..MAX_SYNC_DEPTH {
        let block = match network.request_blocks(source, vec![wanted]).await {
            Ok(blocks) => match blocks.into_iter().next() {
                Some(block) => block,
                None => break,
            },
            Err(error) => {
                eprintln!("block sync from {source} stopped: {error}");
                break;
            }
        };
        let parent = block.header.prev_hash;
        if fetched
            .send(Fetched::Block { source, block })
            .await
            .is_err()
        {
            return;
        }
        if lock_chain(&chain).contains(&parent) {
            break;
        }
        wanted = parent;
    }
    let _ = fetched.send(Fetched::Done(source)).await;
}

/// Applies blocks received over gossip to the local chain.
///
/// Without this the node decodes every gossiped block and then discards it: the
/// P2P layer publishes `BlockReceived` to a broadcast channel that otherwise has
/// no subscriber, so a non-mining peer would sit at genesis forever.
pub(super) async fn block_import_loop(
    chain: Arc<Mutex<Chain>>,
    network: NodeHandle,
    metrics: Arc<Metrics>,
) {
    let mut events = network.subscribe();
    let mut orphans = Orphans::new();
    let (fetched_tx, mut fetched_rx) = mpsc::channel(FETCH_QUEUE);
    let mut walking: HashSet<PeerId> = HashSet::new();

    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(NodeEvent::BlockReceived { block, source, author }) => {
                    // Blame the author, never the relay: see `NodeEvent::BlockReceived`.
                    let report = import_block(&chain, *block, author, &mut orphans, &metrics);
                    report_offences(&network, report.offences).await;
                    // One walk per relay, a few at a time: a relay that keeps
                    // sending orphans cannot fan out walks.
                    if let Some(parent) = report.missing_parent
                        && walking.len() < MAX_CONCURRENT_WALKS
                        && walking.insert(source)
                    {
                        tokio::spawn(walk_ancestors(
                            Arc::clone(&chain),
                            network.clone(),
                            source,
                            parent,
                            fetched_tx.clone(),
                        ));
                    }
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    // Blocks were dropped while this task was behind; backfill will
                    // recover them, so this is a warning rather than a failure.
                    eprintln!("block import lagged, missed {missed} event(s)");
                }
                Err(broadcast::error::RecvError::Closed) => return,
            },
            Some(message) = fetched_rx.recv() => match message {
                // A served block is the responder's to answer for: an honest
                // responder serves only blocks its own chain accepted.
                Fetched::Block { source, block } => {
                    let report = import_block(&chain, block, Some(source), &mut orphans, &metrics);
                    report_offences(&network, report.offences).await;
                }
                Fetched::Done(source) => {
                    walking.remove(&source);
                }
            },
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
pub(super) async fn backfill_loop(chain: Arc<Mutex<Chain>>, url: String, metrics: Arc<Metrics>) {
    let client = match HttpClientBuilder::default().build(&url) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("backfill disabled, bad --sync-from URL {url}: {error}");
            return;
        }
    };

    println!("backfill:    pulling history from {url}");
    let mut orphans = Orphans::new();

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
                    // No peer to blame or ask: the RPC source is trusted for
                    // availability only, and a refused block is still refused.
                    let _ = import_block(&chain, block, None, &mut orphans, &metrics);
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
