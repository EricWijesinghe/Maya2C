//! Getting blocks into the chain: from gossip, and from a peer's history.
//!
//! Split from `node.rs` to keep that file within the project's size limit.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use custom_l1_node::consensus::{Chain, InsertOutcome};
use custom_l1_node::core::Block;
use custom_l1_node::metrics::Metrics;
use custom_l1_node::network::{NodeEvent, NodeHandle};
use custom_l1_node::rpc::BlockInfo;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::HttpClientBuilder;
use jsonrpsee::rpc_params;
use tokio::sync::broadcast;

use super::{lock_chain, unix_now};

/// Pause when backfill has nothing to fetch.
const BACKFILL_IDLE: Duration = Duration::from_secs(2);

/// Maximum buffered blocks whose parent has not arrived.
///
/// Bounded because a peer can send unlimited unattached blocks; an unbounded
/// buffer would be a memory-exhaustion vector.
const MAX_ORPHANS: usize = 512;

/// Imports a block, buffering it if its parent has not arrived yet.
///
/// Gossip does not guarantee ordering, so a child can arrive before its parent.
/// Dropping it would strand the node until the next block it happens to receive
/// in order, so unattached blocks are held and retried once their parent lands.
pub(super) fn import_block(
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
pub(super) async fn backfill_loop(chain: Arc<Mutex<Chain>>, url: String, metrics: Arc<Metrics>) {
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
