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
///
/// Both this assertion and the pre-buffer `check_size` use the compile-time
/// cap, as `insert_block` does. If the cap becomes a scheduled parameter
/// (ADR-147), both must follow the scheduled maximum.
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

/// Buffered blocks one peer may answer for, so filling the pool takes many
/// identities rather than one. A walk buffers at most [`MAX_SYNC_DEPTH`] blocks
/// from its responder, so an honest walk fits. The operator's `--sync-from`
/// source (`None`) is not a network peer and has no share.
const MAX_ORPHANS_PER_SOURCE: usize = MAX_SYNC_DEPTH;
const _: () = assert!(MAX_ORPHANS >= 8 * MAX_ORPHANS_PER_SOURCE);

/// Chain extensions an orphan may wait through before it is dropped. Counted
/// in imports, not wall-clock time, so a halted chain expires nothing and a
/// growing one sheds orphans whose parents never came. Twice a walk's depth,
/// so a walk's blocks are not expired while it is still fetching below them.
const ORPHAN_TTL_IMPORTS: u64 = 2 * MAX_SYNC_DEPTH as u64;

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
    /// Order of arrival, so eviction picks one victim whatever the map order.
    next_seq: u64,
    /// Blocks each network peer answers for ([`MAX_ORPHANS_PER_SOURCE`]).
    per_source: HashMap<PeerId, usize>,
    /// Chain extensions so far, the clock [`ORPHAN_TTL_IMPORTS`] runs on.
    imports: u64,
}

/// One buffered block, with its id and wire size so lookups and removal need
/// neither to be recomputed.
struct Orphan {
    block: Block,
    id: [u8; 32],
    source: Option<PeerId>,
    bytes: u64,
    seq: u64,
    /// `Orphans::imports` when it was buffered.
    at: u64,
}

impl Orphans {
    /// Whether one more block of `bytes` fits under `parent`: the pool's block
    /// count, its byte budget and the per-parent share must all have room.
    #[cfg(test)]
    fn has_room(&self, parent: &[u8; 32], bytes: u64) -> bool {
        self.share_has_room(parent) && self.pool_has_room(bytes)
    }

    fn share_has_room(&self, parent: &[u8; 32]) -> bool {
        self.by_parent.get(parent).map_or(0, Vec::len) < MAX_ORPHANS_PER_PARENT
    }

    fn pool_has_room(&self, bytes: u64) -> bool {
        pool_fits(self.blocks, self.bytes, bytes)
    }

    /// Buffers `block` under `parent`. Returns `false` only when the pool has
    /// no room; a block already held there is not added twice, so repeats from
    /// backfill or an ancestor walk do not use up the parent's share. The id
    /// alone identifies a block here because `import_block` checks `tx_root`
    /// before buffering.
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
            .is_some_and(|siblings| siblings.iter().any(|o| o.id == id));
        if held {
            return true;
        }
        let source_full = source.is_some_and(|peer| {
            self.per_source.get(&peer).copied().unwrap_or(0) >= MAX_ORPHANS_PER_SOURCE
        });
        if source_full || !self.share_has_room(&parent) || !self.make_room(&id, bytes) {
            return false;
        }
        self.blocks += 1;
        self.bytes += bytes;
        self.next_seq += 1;
        if let Some(peer) = source {
            *self.per_source.entry(peer).or_default() += 1;
        }
        self.by_parent.entry(parent).or_default().push(Orphan {
            block,
            id,
            source,
            bytes,
            seq: self.next_seq,
            at: self.imports,
        });
        true
    }

    /// Releases `gone`'s share of the totals; the caller has removed it.
    fn forget(&mut self, gone: &Orphan) {
        self.blocks -= 1;
        self.bytes -= gone.bytes;
        if let Some(peer) = gone.source
            && let Some(count) = self.per_source.get_mut(&peer)
        {
            *count -= 1;
            if *count == 0 {
                self.per_source.remove(&peer);
            }
        }
    }

    /// Counts one chain extension and drops every orphan that has waited
    /// through more than [`ORPHAN_TTL_IMPORTS`] of them. Without this, orphans
    /// under parents that never come held their places until restart, and an
    /// honest out-of-order child found the pool full (audit BFT-01).
    fn note_imports(&mut self, extensions: u64) {
        if extensions == 0 {
            return;
        }
        self.imports += extensions;
        let now = self.imports;
        let mut gone = Vec::new();
        for siblings in self.by_parent.values_mut() {
            gone.extend(siblings.extract_if(.., |o| now - o.at > ORPHAN_TTL_IMPORTS));
        }
        self.by_parent.retain(|_, siblings| !siblings.is_empty());
        for orphan in &gone {
            self.forget(orphan);
        }
    }

    /// Frees room for a block of `bytes` with id `id`, if it needs any.
    ///
    /// Only a block some buffered orphan waits on may displace another: an
    /// ancestor walk fetches newest-first, so without this the top of a long
    /// gap fills the pool and the blocks just above the tip are refused. The
    /// victims are the most recently buffered orphans nothing waits on, the
    /// ones farthest from the tip, never a child of `id` itself.
    fn make_room(&mut self, id: &[u8; 32], bytes: u64) -> bool {
        if self.pool_has_room(bytes) {
            return true;
        }
        if !self.by_parent.contains_key(id) {
            return false;
        }
        let mut leaves: Vec<(u64, [u8; 32], [u8; 32], u64)> = self
            .by_parent
            .iter()
            .filter(|(parent, _)| *parent != id)
            .flat_map(|(parent, siblings)| {
                siblings
                    .iter()
                    .map(move |o| (o.seq, *parent, o.id, o.bytes))
            })
            .filter(|(_, _, child, _)| !self.by_parent.contains_key(child))
            .collect();
        // Newest first; `seq` is unique, so the map's iteration order is not used.
        leaves.sort_unstable_by_key(|leaf| std::cmp::Reverse(leaf.0));
        // Count the victims first, so a block that cannot be made to fit
        // evicts nothing.
        let (mut blocks, mut held, mut victims) = (self.blocks, self.bytes, 0);
        for leaf in &leaves {
            if pool_fits(blocks, held, bytes) {
                break;
            }
            blocks -= 1;
            held -= leaf.3;
            victims += 1;
        }
        if !pool_fits(blocks, held, bytes) {
            return false;
        }
        for (_, parent, child, _) in &leaves[..victims] {
            self.remove(parent, child);
        }
        true
    }

    /// Drops the orphan `child` buffered under `parent`, releasing its share.
    fn remove(&mut self, parent: &[u8; 32], child: &[u8; 32]) {
        let Some(siblings) = self.by_parent.get_mut(parent) else {
            return;
        };
        let gone = siblings
            .iter()
            .position(|o| o.id == *child)
            .map(|index| siblings.swap_remove(index));
        if siblings.is_empty() {
            self.by_parent.remove(parent);
        }
        if let Some(gone) = gone {
            self.forget(&gone);
        }
    }

    /// Removes and returns the blocks waiting on `parent`, releasing their
    /// share of the pool totals.
    fn take_children(&mut self, parent: &[u8; 32]) -> Vec<(Block, Option<PeerId>)> {
        let children = self.by_parent.remove(parent).unwrap_or_default();
        for child in &children {
            self.forget(child);
        }
        children.into_iter().map(|o| (o.block, o.source)).collect()
    }
}

/// Whether a pool holding `blocks` blocks and `held` bytes has room for one
/// more block of `bytes`.
const fn pool_fits(blocks: usize, held: u64, bytes: u64) -> bool {
    blocks < MAX_ORPHANS && held.saturating_add(bytes) <= MAX_ORPHAN_BYTES
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
    let mut extensions = 0;

    while let Some((block, source)) = pending.pop() {
        let id = block.header.id();
        let parent = block.header.prev_hash;
        let header_timestamp = block.header.timestamp;

        let known_parent = {
            let chain = lock_chain(chain);
            chain.contains(&parent)
        };

        if !known_parent {
            // Both checks are stateless, so a bad block is refused here, in
            // `insert_block`'s order, rather than waiting in the pool. The
            // `tx_root` check makes the header id commit to the body, so a copy
            // with another body cannot take the genuine block's place.
            let checked = block.check_tx_root().and_then(|()| block.check_size());
            let bytes = match checked {
                Ok(bytes) => bytes,
                Err(error) => {
                    refuse(&mut report, metrics, source, &error);
                    continue;
                }
            };
            if id == first {
                report.missing_parent = Some(parent);
            }
            // Cap the buffer: an unbounded orphan pool is a memory-exhaustion
            // vector for any peer willing to send junk.
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
                extensions += 1;
                println!("imported block, height now {height}");
                // This block may be the parent something buffered was waiting on.
                pending.extend(orphans.take_children(&id));
            }
            Ok(_) => pending.extend(orphans.take_children(&id)),
            Err(error) => refuse(&mut report, metrics, source, &error),
        }
    }
    // After the loop, so a cascade's children are taken before anything ages.
    orphans.note_imports(extensions);
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
    use custom_l1_node::state::{BlockContext, StateDB};

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
    fn a_copy_with_a_swapped_body_cannot_displace_the_genuine_orphan() {
        let dir = tempfile::tempdir().unwrap();
        let chain = chain(dir.path());
        let mut orphans = Orphans::default();
        let parent = [7u8; 32];
        let genuine = orphan(parent, 0, OUTPUT_WIRE_BYTES);
        // Same header, so the same id, but not the body the header commits to.
        let swapped = Block {
            header: genuine.header.clone(),
            transactions: vec![],
        };

        let report = import(&chain, swapped, Some(PeerId::random()), &mut orphans);
        import(&chain, genuine.clone(), None, &mut orphans);

        assert_eq!(report.offences.len(), 1, "the swapped body was not blamed");
        assert_eq!(
            report.missing_parent, None,
            "an invalid block started a walk"
        );
        let held: Vec<Block> = orphans
            .take_children(&parent)
            .into_iter()
            .map(|(block, _)| block)
            .collect();
        assert_eq!(held, vec![genuine], "the genuine block was lost");
    }

    /// Extends `source` by one empty block and returns it, as a peer ahead of
    /// the local node would serve it.
    fn next_block(source: &Mutex<Chain>, timestamp: u64) -> Block {
        let mut source = source.lock().unwrap();
        let tip = source.tip();
        let mut block = Block::new(
            BlockHeader {
                difficulty_target: source.next_target(&tip).unwrap(),
                timestamp,
                ..header(tip, 0)
            },
            vec![],
        );
        block.header.state_root = source
            .state()
            .preview_root(&block, BlockContext::at_height(source.height() + 1))
            .unwrap();
        source.insert_block(block.clone()).unwrap();
        block
    }

    #[test]
    fn a_walk_into_a_nearly_full_pool_still_lands_a_run_of_blocks() {
        const GAP: usize = 20;
        const ROOM: usize = 5;
        let (source_dir, local_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let source = chain(source_dir.path());
        let local = chain(local_dir.path());
        let gap: Vec<Block> = (0..GAP as u64)
            .map(|i| next_block(&source, 2 + i))
            .collect();

        let mut orphans = Orphans::default();
        // Unrelated orphans hold all but ROOM places in the pool.
        for index in 0..(MAX_ORPHANS - ROOM) as u64 {
            let mut parent = [0xee; 32];
            parent[..8].copy_from_slice(&index.to_be_bytes());
            import(&local, orphan(parent, 0, 0), None, &mut orphans);
        }

        // Each walk fetches newest-first, from the top of the gap down to the
        // first block whose parent the node holds.
        let mut walks = 0;
        while lock_chain(&local).height() < GAP as u64 && walks < GAP {
            walks += 1;
            let height = usize::try_from(lock_chain(&local).height()).unwrap();
            for block in gap[height..].iter().rev() {
                import(&local, block.clone(), None, &mut orphans);
            }
        }
        assert_eq!(
            lock_chain(&local).height(),
            GAP as u64,
            "the tip never reached the top"
        );
        // Each walk lands its bottom block and the ROOM above it.
        assert_eq!(
            walks,
            GAP.div_ceil(ROOM + 1),
            "a walk landed only its bottom block"
        );
    }

    #[test]
    fn an_ancestor_that_cannot_be_made_to_fit_evicts_nothing() {
        let mut orphans = Orphans::default();
        let ancestor = orphan([1u8; 32], 0, 0);
        let awaited = ancestor.header.id();
        // Its own child fills most of the budget and is never a victim; the one
        // other orphan is too small to make room.
        assert!(orphans.buffer(awaited, orphan(awaited, 1, 0), None, MAX_ORPHAN_BYTES - 20));
        assert!(orphans.buffer([2u8; 32], orphan([2u8; 32], 2, 0), None, 10));

        assert!(!orphans.buffer([1u8; 32], ancestor, None, 100));
        assert_eq!(orphans.blocks, 2, "an orphan was dropped for nothing");
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

    /// An orphan under a distinct, made-up parent.
    fn junk(index: u64) -> ([u8; 32], Block) {
        let mut parent = [0xaa; 32];
        parent[..8].copy_from_slice(&index.to_be_bytes());
        (parent, orphan(parent, index, 0))
    }

    #[test]
    fn one_source_cannot_fill_the_pool_and_an_honest_child_still_buffers() {
        let mut orphans = Orphans::default();
        let flooder = Some(PeerId::random());
        // One peer sends a pool's worth of orphans under unknown parents.
        for index in 0..MAX_ORPHANS as u64 {
            let (parent, block) = junk(index);
            orphans.buffer(parent, block, flooder, 1);
        }
        assert!(
            orphans.blocks <= MAX_ORPHANS_PER_SOURCE,
            "one source holds {} of {MAX_ORPHANS} places",
            orphans.blocks
        );
        let honest_parent = [0x55; 32];
        assert!(
            orphans.buffer(
                honest_parent,
                orphan(honest_parent, 0, 0),
                Some(PeerId::random()),
                1
            ),
            "an honest out-of-order child was refused"
        );
    }

    #[test]
    fn orphans_nothing_claims_expire_after_enough_imports() {
        let (source_dir, local_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let source = chain(source_dir.path());
        let local = chain(local_dir.path());
        let mut orphans = Orphans::default();
        // Many identities fill the pool with orphans whose parents never come.
        for index in 0..MAX_ORPHANS as u64 {
            let (_, block) = junk(index);
            import(&local, block, Some(PeerId::random()), &mut orphans);
        }
        assert_eq!(orphans.blocks, MAX_ORPHANS, "the pool did not fill");

        // The chain keeps growing from blocks that arrive in order.
        for i in 0..=ORPHAN_TTL_IMPORTS {
            let report = import(&local, next_block(&source, 2 + i), None, &mut orphans);
            assert!(report.missing_parent.is_none(), "an in-order block was orphaned");
        }
        assert_eq!(orphans.blocks, 0, "stale orphans outlived the import window");
        assert_eq!(orphans.bytes, 0, "expiry left bytes counted");
        let (parent, block) = junk(u64::MAX);
        assert!(
            orphans.buffer(parent, block, Some(PeerId::random()), 1),
            "the pool stayed full after expiry"
        );
    }
}
