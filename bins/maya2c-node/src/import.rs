//! Getting blocks into the chain: from gossip, and from a peer's history.
//!
//! Split from `node.rs` to keep that file within the project's size limit.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use custom_l1_node::consensus::{Chain, InsertOutcome};
use custom_l1_node::core::Block;
use custom_l1_node::error::NodeError;
use custom_l1_node::metrics::Metrics;
use custom_l1_node::network::peer_health::classify_import;
use custom_l1_node::network::{NodeEvent, NodeHandle, Offence};
use custom_l1_node::rpc::BlockInfo;
use custom_l1_node::state::fees::MAX_BLOCK_BYTES;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::HttpClientBuilder;
use jsonrpsee::rpc_params;
use libp2p::PeerId;
use tokio::sync::{broadcast, mpsc};

use super::{lock_chain, unix_now};

/// Pause when backfill has nothing to fetch.
const BACKFILL_IDLE: Duration = Duration::from_secs(2);

/// Maximum buffered blocks whose parent has not arrived, counted as blocks.
///
/// One of two bounds on the pool; [`MAX_ORPHAN_BYTES`] is the other. A count
/// alone would let large blocks hold `MAX_ORPHANS` times the gossip cap.
const MAX_ORPHANS: usize = 512;

/// Wire bytes the orphan pool may hold in all. Together with the per-block
/// size check before buffering, this bounds the memory held for unattached
/// blocks whatever the gossip volume. Twelve maximum-size blocks fit.
const MAX_ORPHAN_BYTES: u64 = 64 * 1024 * 1024;
const _: () = assert!(MAX_ORPHAN_BYTES >= MAX_BLOCK_BYTES);

/// Buffered blocks allowed under one missing parent, so one parent id cannot
/// hold the whole pool. Assumes honest DAG-BFT blocks form a linear
/// `prev_hash` chain: siblings sharing a parent beyond this are refused from
/// the buffer, so raise it if a legitimate fork can exceed eight.
const MAX_ORPHANS_PER_PARENT: usize = 8;

/// Parents fetched in a row for one orphan before giving up. A peer that keeps
/// answering with yet another unknown parent is either very far ahead or
/// leading the node down a chain nobody else has.
const MAX_SYNC_DEPTH: usize = 64;

/// Buffered blocks waiting on a parent, keyed by that parent, with the peer to
/// blame if each proves invalid: a gossiped block's author, a fetched block's
/// responder — never a relay (see `NodeEvent::BlockReceived`).
///
/// Keeps running block and byte totals so both pool bounds are checked
/// without a scan. Looked up by key only; its iteration order is never used.
#[derive(Default)]
pub(super) struct Orphans {
    by_parent: HashMap<[u8; 32], Vec<Orphan>>,
    blocks: usize,
    bytes: u64,
}

/// One buffered block, with its wire size so removal can return it.
struct Orphan {
    block: Block,
    source: Option<PeerId>,
    bytes: u64,
}

impl Orphans {
    /// Whether one more block of `bytes` fits under `parent`: the pool's block
    /// count, its byte budget and the per-parent share must all have room.
    fn has_room(&self, parent: &[u8; 32], bytes: u64) -> bool {
        let under_parent = self.by_parent.get(parent).map_or(0, Vec::len);
        self.blocks < MAX_ORPHANS
            && self.bytes.saturating_add(bytes) <= MAX_ORPHAN_BYTES
            && under_parent < MAX_ORPHANS_PER_PARENT
    }

    /// Buffers `block` under `parent`. Returns `false` only when the pool has
    /// no room; a block already held there is not added twice, so repeats from
    /// backfill or an ancestor walk do not use up the parent's share.
    fn buffer(
        &mut self,
        parent: [u8; 32],
        block: Block,
        source: Option<PeerId>,
        bytes: u64,
    ) -> bool {
        let id = block.header.id();
        let held = self
            .by_parent
            .get(&parent)
            .is_some_and(|siblings| siblings.iter().any(|o| o.block.header.id() == id));
        if held {
            return true;
        }
        if !self.has_room(&parent, bytes) {
            return false;
        }
        self.blocks += 1;
        self.bytes += bytes;
        self.by_parent.entry(parent).or_default().push(Orphan {
            block,
            source,
            bytes,
        });
        true
    }

    /// Removes and returns the blocks waiting on `parent`, releasing their
    /// share of both pool totals.
    fn take_children(&mut self, parent: &[u8; 32]) -> Vec<(Block, Option<PeerId>)> {
        let children = self.by_parent.remove(parent).unwrap_or_default();
        self.blocks -= children.len();
        self.bytes -= children.iter().map(|o| o.bytes).sum::<u64>();
        children.into_iter().map(|o| (o.block, o.source)).collect()
    }
}

/// What an import found that the network layer must hear about.
#[derive(Default)]
pub(super) struct ImportReport {
    /// Refused blocks, each against the peer to blame for it.
    pub(super) offences: Vec<(PeerId, Offence)>,
    /// The imported block's parent, if it is unknown. Set whether or not the
    /// pool had room, so a full share under that parent still starts a walk.
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
            // The size check is stateless, so an oversized block is refused
            // here; it would otherwise wait in the pool for `insert_block`.
            if let Err(error) = block.check_size() {
                refuse(&mut report, metrics, source, &error);
                continue;
            }
            if id == first {
                report.missing_parent = Some(parent);
            }
            // Cap the buffer: an unbounded orphan pool is a memory-exhaustion
            // vector for any peer willing to send junk.
            let bytes = block.to_bytes().len() as u64;
            if !orphans.buffer(parent, block, source, bytes) {
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
                pending.extend(orphans.take_children(&id));
            }
            Ok(_) => pending.extend(orphans.take_children(&id)),
            Err(error) => refuse(&mut report, metrics, source, &error),
        }
    }
    report
}

/// Counts a refused block and blames `source` if the error is an offence.
fn refuse(report: &mut ImportReport, metrics: &Metrics, source: Option<PeerId>, error: &NodeError) {
    // The label is a fixed string, never the error text: a label drawn from an
    // attacker-influenced message would let one peer create unbounded time
    // series in every scraper on the network.
    metrics.record_rejection("invalid");
    eprintln!("rejected an imported block: {error}");
    if let (Some(peer), Some(offence)) = (source, classify_import(error)) {
        report.offences.push((peer, offence));
    }
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
/// [`import_block`] and the capped orphan buffer like gossip does, so
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
    let mut orphans = Orphans::default();
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
    let mut orphans = Orphans::default();

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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use custom_l1_node::consensus::chain::ChainConfig;
    use custom_l1_node::core::{BlockHeader, Transaction, TxOutput};
    use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
    use custom_l1_node::state::StateDB;

    /// Wire bytes one `TxOutput` adds: a 32-byte amount and a 32-byte address.
    const OUTPUT_WIRE_BYTES: usize = 64;

    fn header(parent: [u8; 32], nonce: u64) -> BlockHeader {
        BlockHeader {
            version: 0,
            prev_hash: parent,
            state_root: [0; 32],
            timestamp: 1,
            nonce,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        }
    }

    /// A block under `parent` whose wire form is about `wire_bytes` long.
    /// Unsigned: it only has to be buffered, never applied.
    fn orphan(parent: [u8; 32], nonce: u64, wire_bytes: usize) -> Block {
        let outputs = vec![
            TxOutput {
                amount: primitive_types::U256::one(),
                recipient: [1; 32],
            };
            wire_bytes / OUTPUT_WIRE_BYTES
        ];
        let txs = if outputs.is_empty() {
            vec![]
        } else {
            vec![Transaction::new(vec![], outputs, 0)]
        };
        Block::new(header(parent, nonce), txs)
    }

    fn chain(dir: &std::path::Path) -> Mutex<Chain> {
        let state = Arc::new(StateDB::open(dir).unwrap());
        let genesis = Block::new(header([0; 32], 0), vec![]);
        Mutex::new(Chain::open(state, genesis, ChainConfig::without_pow_verification()).unwrap())
    }

    fn import(
        chain: &Mutex<Chain>,
        block: Block,
        source: Option<PeerId>,
        orphans: &mut Orphans,
    ) -> ImportReport {
        import_block(chain, block, source, orphans, &Metrics::new())
    }

    #[test]
    fn a_ninth_orphan_under_one_parent_is_refused_by_import() {
        let dir = tempfile::tempdir().unwrap();
        let chain = chain(dir.path());
        let mut orphans = Orphans::default();
        let parent = [7u8; 32];
        for nonce in 0..=MAX_ORPHANS_PER_PARENT as u64 {
            import(&chain, orphan(parent, nonce, 0), None, &mut orphans);
        }
        assert_eq!(
            orphans.blocks, MAX_ORPHANS_PER_PARENT,
            "per-parent share not enforced"
        );
        import(&chain, orphan([8u8; 32], 0, 0), None, &mut orphans);
        assert_eq!(
            orphans.blocks,
            MAX_ORPHANS_PER_PARENT + 1,
            "another parent is refused"
        );
    }

    #[test]
    fn a_refused_orphan_still_names_its_missing_parent() {
        let dir = tempfile::tempdir().unwrap();
        let chain = chain(dir.path());
        let mut orphans = Orphans::default();
        let parent = [7u8; 32];
        for nonce in 0..MAX_ORPHANS_PER_PARENT as u64 {
            import(&chain, orphan(parent, nonce, 0), None, &mut orphans);
        }
        let report = import(&chain, orphan(parent, 99, 0), None, &mut orphans);
        assert_eq!(
            orphans.blocks, MAX_ORPHANS_PER_PARENT,
            "the share was not full"
        );
        assert_eq!(report.missing_parent, Some(parent), "no walk would start");
    }

    #[test]
    fn a_repeated_orphan_does_not_use_up_the_parent_share() {
        let dir = tempfile::tempdir().unwrap();
        let chain = chain(dir.path());
        let mut orphans = Orphans::default();
        let parent = [7u8; 32];
        for _ in 0..=MAX_ORPHANS_PER_PARENT {
            import(&chain, orphan(parent, 1, 0), None, &mut orphans);
        }
        assert_eq!(orphans.blocks, 1, "a repeat was buffered twice");
        import(&chain, orphan(parent, 2, 0), None, &mut orphans);
        assert_eq!(orphans.blocks, 2, "a new sibling was refused");
    }

    #[test]
    fn an_oversized_orphan_is_refused_and_blamed_before_buffering() {
        let dir = tempfile::tempdir().unwrap();
        let chain = chain(dir.path());
        let mut orphans = Orphans::default();
        let author = PeerId::random();
        let big = orphan(
            [7u8; 32],
            0,
            usize::try_from(MAX_BLOCK_BYTES).unwrap() + 1024,
        );
        assert!(big.check_size().is_err(), "fixture is not over the cap");

        let report = import(&chain, big, Some(author), &mut orphans);
        assert_eq!(orphans.blocks, 0, "an oversized block was buffered");
        assert_eq!(report.offences.len(), 1, "the author was not blamed");
        assert_eq!(
            report.missing_parent, None,
            "an invalid block started a walk"
        );
    }

    #[test]
    fn the_pool_refuses_at_its_byte_budget_before_its_block_count() {
        let dir = tempfile::tempdir().unwrap();
        let chain = chain(dir.path());
        let mut orphans = Orphans::default();
        let size = 4 * 1024 * 1024;
        let fits = usize::try_from(MAX_ORPHAN_BYTES).unwrap() / size;
        for index in 0..=fits as u64 {
            let mut parent = [0u8; 32];
            parent[..8].copy_from_slice(&index.to_be_bytes());
            import(&chain, orphan(parent, 0, size), None, &mut orphans);
        }
        assert!(
            orphans.blocks < MAX_ORPHANS,
            "the block count refused first"
        );
        assert!(
            orphans.bytes <= MAX_ORPHAN_BYTES,
            "byte budget exceeded: {}",
            orphans.bytes
        );
        assert!(orphans.blocks <= fits, "every large block was buffered");
    }

    #[test]
    fn taking_children_releases_their_share_of_both_totals() {
        let mut orphans = Orphans::default();
        let parent = [7u8; 32];
        assert!(orphans.buffer(parent, orphan(parent, 0, 0), None, 1_000));
        assert!(orphans.buffer(parent, orphan(parent, 1, 0), None, 2_000));
        assert_eq!(orphans.take_children(&parent).len(), 2);
        assert_eq!(
            (orphans.blocks, orphans.bytes),
            (0, 0),
            "totals not released"
        );
    }

    #[test]
    fn the_pool_limit_counts_blocks_not_parents() {
        // 64 parents x 8 blocks: 512 blocks under 64 keys. A key count would
        // still admit here (64 < 512), so only block counting refuses it.
        let mut orphans = Orphans::default();
        for parent_index in 0..64u64 {
            let mut parent = [0u8; 32];
            parent[..8].copy_from_slice(&parent_index.to_be_bytes());
            for nonce in 0..MAX_ORPHANS_PER_PARENT as u64 {
                let block = orphan(parent, parent_index * 8 + nonce, 0);
                assert!(orphans.buffer(parent, block, None, 1));
            }
        }
        assert_eq!(
            orphans.blocks, MAX_ORPHANS,
            "pool not filled to its block limit"
        );
        assert!(!orphans.has_room(&[0xff; 32], 1), "full pool still admits");
    }
}
