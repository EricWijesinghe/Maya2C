//! Catching up to a checkpoint (ADR-038, mainnet gate 8).
//!
//! A node that fell more than the engine's 50-round window behind cannot
//! derive the blocks it missed. It asks a peer for the newest checkpoint —
//! a block a quorum of the committee attested — checks the quorum against
//! the committee it already knows, fetches the missing blocks, checks they
//! hash-chain to the attested block, and **re-executes** each through
//! [`Chain::insert_block`], which refuses a state root its own execution
//! does not reproduce (invariant 24). The peer is trusted for nothing: the
//! quorum names the chain, re-execution checks the state. Afterwards the
//! node is within the window and the engine fetches the last rounds itself.

use crate::consensus::{Chain, InsertOutcome};
use crate::core::{Block, ChainTag};
use crate::crypto::validator_keys::VerifyingKey;
use crate::error::{NodeError, Result};
use crate::state_pruner::snapshot::TrustedBlock;

use super::attest::Checkpoint;

/// Where a node that fell behind gets a checkpoint and the blocks up to it.
/// Every answer is untrusted; [`catch_up`] checks all of it.
/// `Sync` because blocks are fetched with several requests in flight
/// ([`crate::state_pruner::prefetch`]).
pub trait CheckpointSource: Sync {
    /// The source's newest checkpoint, if it holds one.
    ///
    /// # Errors
    ///
    /// Any failure reaching the source.
    fn checkpoint(&self) -> Result<Option<Checkpoint>>;

    /// The source's newest checkpoint of `epoch`, if it still holds one:
    /// how a node behind an epoch boundary crosses it (see [`fetch`]).
    ///
    /// # Errors
    ///
    /// Any failure reaching the source.
    fn checkpoint_of(&self, epoch: u64) -> Result<Option<Checkpoint>>;

    /// The active-chain block at `height`.
    ///
    /// # Errors
    ///
    /// Any failure reaching the source.
    fn block(&self, height: u64) -> Result<Block>;

    /// The active-chain blocks `from..=to`, in order, at most
    /// [`BLOCKS_PER_REQUEST`] of them. Catch-up is bound by round trips, not
    /// bandwidth, so a source that can answer a range in one request should
    /// (the RPC source does); the default asks one by one, which is what a
    /// peer without the batch method gets. Fewer blocks than asked is an
    /// error to [`fetch`], never a gap it skips.
    ///
    /// # Errors
    ///
    /// Any failure reaching the source.
    fn blocks(&self, from: u64, to: u64) -> Result<Vec<Block>> {
        (from..=to).map(|h| self.block(h)).collect()
    }
}

/// Blocks asked for in one request. A peer hours behind needs thousands, and
/// at ~0.7 s a round trip one at a time ran at about 5 blocks/s (measured
/// 2026-10-24); 32 per request keeps a response far under the RPC body limit.
pub const BLOCKS_PER_REQUEST: u64 = 32;

/// Chunks fetched before the caller links them: bounds memory, and keeps
/// [`crate::state_pruner::prefetch::PREFETCH_WORKERS`] busy.
const CHUNK_WINDOW: u64 = 16;

/// Where this node's chain stands, read under the chain lock so the slow
/// part — fetching from a peer — can run without it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    /// The chain the checkpoint must name.
    pub tag: ChainTag,
    /// Current height.
    pub height: u64,
    /// Current tip.
    pub tip: [u8; 32],
    /// The staking epoch the chain's state is in: whose committee this node
    /// trusts. Zero on a chain without staking.
    pub epoch: u64,
}

impl Position {
    /// `chain`'s position now.
    #[must_use]
    pub fn of(chain: &Chain) -> Self {
        Self {
            tag: ChainTag::from_genesis(chain.genesis()),
            height: chain.height(),
            tip: chain.tip(),
            // A state that cannot be read here only names a wrong epoch, and
            // a checkpoint fetched for it fails verification: safe.
            epoch: chain
                .state()
                .committed_staking()
                .ok()
                .flatten()
                .map_or(0, |r| r.staking.epoch),
        }
    }
}

/// Imports blocks from `source` up to its newest checkpoint, returning how
/// many were imported (zero when already there).
///
/// `committee` is the committee this node already trusts — the genesis or
/// staking committee in its own state. A checkpoint signed by another
/// committee is refused. To cross a membership change, [`fetch`] stops at the
/// final checkpoint of this node's own epoch: the boundary block below it
/// writes the next committee into state, and the next call trusts that
/// one. Call again until nothing is imported. `weights` are that
/// committee's voting weights on a stake-weighted chain (ADR-040 part 2),
/// `None` where every member counts one.
///
/// # Errors
///
/// As [`fetch`] and [`import`].
pub fn catch_up(
    chain: &mut Chain,
    source: &impl CheckpointSource,
    committee: &[VerifyingKey],
    weights: Option<&[u64]>,
) -> Result<u64> {
    let blocks = fetch(Position::of(chain), source, committee, weights)?;
    import(chain, blocks).map(|report| report.applied)
}

/// The final checkpoint of `from`'s own epoch, for crossing into the next
/// one. Loud rather than empty when it cannot move this node forward: an
/// empty answer would leave a node at an epoch boundary looking caught up
/// while it waits for a boundary block it never imports.
fn own_epoch_final(
    from: Position,
    source: &impl CheckpointSource,
    committee: &[VerifyingKey],
    weights: Option<&[u64]>,
) -> Result<Checkpoint> {
    let Some(own) = source.checkpoint_of(from.epoch)? else {
        return Err(NodeError::Network(format!(
            "the peer holds no checkpoint of epoch {}, which it keeps for {} epochs:              restore from a snapshot (--bootstrap-from) instead",
            from.epoch,
            super::attest::KEPT_EPOCHS
        )));
    };
    if own.epoch != from.epoch {
        return Err(NodeError::Network(format!(
            "asked for epoch {}'s final checkpoint, the peer sent one of epoch {}",
            from.epoch, own.epoch
        )));
    }
    own.verify(&from.tag, committee, weights)?;
    if own.height <= from.height {
        return Err(NodeError::Network(format!(
            "stuck at the end of epoch {}: the peer's final checkpoint of it is at height {},              not past this node's {}; try another peer",
            from.epoch, own.height, from.height
        )));
    }
    Ok(own)
}

/// Fetches the blocks after `from` up to `source`'s newest checkpoint, checked
/// to carry a quorum of `committee` for `from.tag` and to form one hash chain
/// from `from.tip` to the attested block. Empty when there is nothing newer.
///
/// # Errors
///
/// [`NodeError::Decode`] for a checkpoint without a quorum or blocks that do
/// not chain to it; errors from `source` as they come.
pub fn fetch(
    from: Position,
    source: &impl CheckpointSource,
    committee: &[VerifyingKey],
    weights: Option<&[u64]>,
) -> Result<Vec<Block>> {
    let Some(newest) = source.checkpoint()? else {
        return Ok(Vec::new());
    };
    // A checkpoint is only checked against its own epoch's committee
    // (pre-audit S-011). The network past this node's epoch: go as far as this
    // epoch's own final checkpoint first. This node past the network's newest
    // checkpoint (it imported the boundary block, but the new committee has
    // signed nothing yet): there is nothing it can check, so nothing to import.
    let checkpoint = match newest.epoch.cmp(&from.epoch) {
        core::cmp::Ordering::Greater => own_epoch_final(from, source, committee, weights)?,
        core::cmp::Ordering::Equal => newest,
        core::cmp::Ordering::Less => return Ok(Vec::new()),
    };
    checkpoint.verify(&from.tag, committee, weights)?;
    fetch_linked(
        from,
        checkpoint.height,
        checkpoint.block,
        source,
        "attested",
        None,
    )
}

/// Fetches the blocks after `from` up to height `to_height`, checked to form
/// one hash chain from `from.tip` to the block `to_id` there. Empty when
/// `to_height` is not past `from`. `what` names the block in the error.
///
/// # Errors
///
/// [`NodeError::Decode`] for blocks that do not chain to `to_id`; errors from
/// `source` as they come.
fn fetch_linked(
    from: Position,
    to_height: u64,
    to_id: [u8; 32],
    source: &impl CheckpointSource,
    what: &str,
    max_bytes: Option<u64>,
) -> Result<Vec<Block>> {
    if to_height <= from.height {
        return Ok(Vec::new());
    }
    let mut parent = from.tip;
    let mut height = from.height;
    let mut blocks = Vec::new();
    // Fetched in chunks with several requests in flight, linked strictly in
    // order: a source that answers out of order or lies gains nothing.
    let first = from.height + 1;
    let last = to_height;
    let chunks = (last - first) / BLOCKS_PER_REQUEST + 1;
    let range = |i: u64| {
        let lo = first + i * BLOCKS_PER_REQUEST;
        (lo, last.min(lo + BLOCKS_PER_REQUEST - 1))
    };
    // Encoded bytes received so far, across every chunk. Checked as each
    // response arrives, so the fetch stops mid-gap rather than after it.
    let received = std::sync::atomic::AtomicU64::new(0);
    for start in (0..chunks).step_by(usize::try_from(CHUNK_WINDOW).unwrap_or(1)) {
        let end = (start + CHUNK_WINDOW - 1).min(chunks - 1);
        let window = crate::state_pruner::prefetch::fetch_ordered(
            start,
            end,
            crate::state_pruner::prefetch::PREFETCH_WORKERS,
            &|i| {
                let (lo, hi) = range(i);
                let got = source.blocks(lo, hi)?;
                if got.len() as u64 != hi - lo + 1 {
                    return Err(NodeError::Network(format!(
                        "catch-up: asked for blocks {lo}..={hi}, the peer sent {}",
                        got.len()
                    )));
                }
                if let Some(limit) = max_bytes {
                    let bytes: u64 = got
                        .iter()
                        .map(|b| u64::try_from(b.to_bytes().len()).unwrap_or(u64::MAX))
                        .sum();
                    let total = received
                        .fetch_add(bytes, std::sync::atomic::Ordering::SeqCst)
                        .saturating_add(bytes);
                    if total > limit {
                        return Err(gap_bytes_error(total, limit));
                    }
                }
                Ok(got)
            },
        )?;
        for block in window.into_iter().flatten() {
            height += 1;
            if block.header.prev_hash != parent {
                return Err(NodeError::Decode(format!(
                    "catch-up: block {height} does not extend the one before it"
                )));
            }
            parent = block.header.id();
            blocks.push(block);
        }
    }
    if parent != to_id {
        return Err(NodeError::Decode(format!(
            "catch-up: the blocks do not lead to the {what} block at height {to_height}"
        )));
    }
    Ok(blocks)
}

/// How a catch-up batch is checked against a trusted block. Node policy, not
/// a consensus rule: block validity and state roots are the same under every
/// variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    /// The committee the node already trusts decides, as before.
    Unanchored,
    /// Ordinary catch-up: a batch that reaches the trusted height must carry
    /// the trusted id there.
    Checked(TrustedBlock),
    /// A node stale past the unbonding window. Its committee may have
    /// unbonded and be forging a chain, so the gap below the trusted block is
    /// linked to it by hash alone; the committee rule resumes above it.
    Stale(TrustedBlock),
}

/// The next batch for `from` under `anchor`, as [`fetch`] would return it,
/// except where the anchor says otherwise.
///
/// # Errors
///
/// As [`fetch`], and [`NodeError::Decode`] when the blocks do not match the
/// trusted block (nothing is returned, so nothing is imported).
pub fn fetch_anchored(
    from: Position,
    source: &impl CheckpointSource,
    committee: &[VerifyingKey],
    weights: Option<&[u64]>,
    anchor: Anchor,
) -> Result<Vec<Block>> {
    match anchor {
        Anchor::Unanchored => fetch(from, source, committee, weights),
        Anchor::Checked(trusted) if from.height < trusted.height => {
            let blocks = fetch(from, source, committee, weights)?;
            check_trusted(from, &blocks, trusted)?;
            Ok(blocks)
        }
        // At or past the trusted height: a batch that crossed it was checked
        // then, and a block at or below the tip at start-up is checked by the
        // node's own start-up rule (`rejoin::check_against_tip`).
        Anchor::Checked(_) => fetch(from, source, committee, weights),
        Anchor::Stale(trusted) if from.height < trusted.height => {
            check_gap(from.height, trusted.height)?;
            fetch_linked(
                from,
                trusted.height,
                trusted.id,
                source,
                "trusted",
                Some(MAX_LINKED_GAP_BYTES),
            )
        }
        // Past the trusted block the blocks just linked have written the
        // current committee into state, so the committee rule applies again.
        Anchor::Stale(_) => fetch(from, source, committee, weights),
    }
}

/// Most blocks a stale node may link across below its trusted block. Why: the
/// whole gap is buffered before the trusted id can be compared, so the cap
/// bounds the memory a hostile peer can make a stale node hold. 50,000 blocks
/// is about 8.7 days at the 15 s target. A gap past it means the operator
/// picked a trusted block far above the tip.
/// Shortcut: the cap counts blocks, not bytes, and the bytes per block are not
/// bounded here. Upgrade: a byte cap once the encoded block size limit is a
/// named constant this path can read.
pub const MAX_LINKED_GAP: u64 = 50_000;

/// Most encoded bytes a stale node may receive while linking across its gap.
/// Why: the cap in blocks alone does not bound memory, since one block can be
/// up to `MAX_BLOCK_BYTES` (5 MiB, ADR-072). 512 MiB admits about 102 blocks
/// of that maximum size, and about 10.7 KiB per block across the full
/// 50,000-block cap, so the byte cap is the binding one for large blocks. The
/// check runs as each response arrives, so memory stays within this plus one
/// request of [`BLOCKS_PER_REQUEST`] blocks.
pub const MAX_LINKED_GAP_BYTES: u64 = 512 * 1024 * 1024;

/// Refuses a stale node's link across more than [`MAX_LINKED_GAP`] blocks.
///
/// # Errors
///
/// [`NodeError::Decode`] naming the gap and asking for a closer trusted block.
pub fn check_gap(from_height: u64, trusted_height: u64) -> Result<()> {
    let gap = trusted_height.saturating_sub(from_height);
    if gap <= MAX_LINKED_GAP {
        return Ok(());
    }
    Err(NodeError::Decode(format!(
        "catch-up: the gap below the trusted block is {gap} blocks, over the {MAX_LINKED_GAP}-block cap. Choose a trusted block closer to this node's tip (above it, within the unbonding window of the head), or, if this node was offline longer than that, restore from a snapshot with --bootstrap-from"
    )))
}

/// The byte-cap refusal, once the running total passes `limit`.
fn gap_bytes_error(total: u64, limit: u64) -> NodeError {
    NodeError::Decode(format!(
        "catch-up: the blocks below the trusted block exceed {limit} bytes ({total} received so far). Choose a trusted block closer to this node's tip, or, if this node was offline longer than the window allows, restore from a snapshot with --bootstrap-from"
    ))
}

/// Checks the block `blocks` carry at `trusted.height`, if they reach it.
/// A batch that stops short checks nothing; the batch that reaches the height
/// does.
///
/// # Errors
///
/// [`NodeError::Decode`] when the block at that height is not `trusted.id`.
pub fn check_trusted(from: Position, blocks: &[Block], trusted: TrustedBlock) -> Result<()> {
    let Some(index) = trusted.height.checked_sub(from.height + 1) else {
        return Ok(());
    };
    let Some(block) = usize::try_from(index).ok().and_then(|i| blocks.get(i)) else {
        return Ok(());
    };
    if block.header.id() == trusted.id {
        return Ok(());
    }
    Err(NodeError::Decode(format!(
        "catch-up: the block at height {} is not the trusted block: the peer serves another chain; importing nothing",
        trusted.height
    )))
}

/// What [`import`] did with a batch.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Imported {
    /// Blocks that extended the tip.
    pub applied: u64,
    /// Blocks the chain already held. A batch that overlaps what we have is
    /// normal, so this is not a fault.
    pub already_held: u64,
}

/// Re-executes `blocks` onto `chain` through [`Chain::insert_block`], which
/// refuses a state root its own execution does not reproduce (invariant 24).
///
/// # Errors
///
/// A block that fails to execute, or one that is neither an extension of the
/// tip nor already held — the chain moved since [`fetch`]. Blocks imported
/// before it stay.
pub fn import(chain: &mut Chain, blocks: Vec<Block>) -> Result<Imported> {
    let mut report = Imported::default();
    for block in blocks {
        match chain.insert_block(block)? {
            InsertOutcome::Extended { .. } => report.applied += 1,
            InsertOutcome::Duplicate { .. } => report.already_held += 1,
            other => {
                return Err(NodeError::Decode(format!(
                    "catch-up: a block did not extend the tip: {other:?}"
                )));
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use super::*;
    use crate::consensus::bft::attest::Attestation;
    use crate::crypto::validator_keys::{SigningKey, signing_key_from_seed};

    const TAG: ChainTag = ChainTag::from_genesis([7; 32]);

    fn keys() -> (Vec<SigningKey>, Vec<VerifyingKey>) {
        let signing: Vec<SigningKey> = (1..=4u8)
            .map(|i| signing_key_from_seed(&[i; 32]).unwrap())
            .collect();
        let verifying = signing.iter().map(SigningKey::verifying_key).collect();
        (signing, verifying)
    }

    /// Three of four sign: a quorum.
    fn checkpoint(signing: &[SigningKey], epoch: u64, height: u64) -> Checkpoint {
        let signatures = (0..3u16)
            .map(|v| {
                let a =
                    Attestation::sign(&signing[usize::from(v)], v, &TAG, epoch, height, [9; 32])
                        .unwrap();
                (v, a.signature)
            })
            .collect::<BTreeMap<_, _>>();
        Checkpoint {
            epoch,
            height,
            block: [9; 32],
            signatures,
        }
    }

    struct Source {
        newest: Checkpoint,
        of_epoch: Option<Checkpoint>,
        asked: Mutex<Vec<u64>>,
    }

    impl CheckpointSource for Source {
        fn checkpoint(&self) -> Result<Option<Checkpoint>> {
            Ok(Some(self.newest.clone()))
        }
        fn checkpoint_of(&self, epoch: u64) -> Result<Option<Checkpoint>> {
            self.asked.lock().expect("lock").push(epoch);
            Ok(self.of_epoch.clone())
        }
        fn block(&self, height: u64) -> Result<Block> {
            Err(NodeError::Network(format!("block {height} requested")))
        }
    }

    fn at(height: u64, epoch: u64) -> Position {
        Position {
            tag: TAG,
            height,
            tip: [0; 32],
            epoch,
        }
    }

    #[test]
    fn behind_a_boundary_it_fetches_up_to_its_own_epochs_final_checkpoint() {
        let (signing, committee) = keys();
        let source = Source {
            newest: checkpoint(&signing, 2, 90), // a committee it cannot check
            of_epoch: Some(checkpoint(&signing, 1, 60)),
            asked: Mutex::default(),
        };
        let err = fetch(at(40, 1), &source, &committee, None).unwrap_err();
        assert_eq!(
            *source.asked.lock().expect("lock"),
            [1],
            "asked for its own epoch's final checkpoint"
        );
        assert!(err.to_string().contains("block 41 requested"), "{err}");
    }

    #[test]
    fn past_the_newest_checkpoints_epoch_it_imports_nothing_instead_of_misverifying() {
        // Pre-audit S-011 (attacknet round 8): the node imported the boundary
        // block, so its state is in epoch 2, while the peer's newest checkpoint
        // is still epoch 1's. Checking it against epoch 2's committee refused a
        // valid checkpoint as "short of a quorum"; it is simply not newer.
        let (signing, committee) = keys();
        let source = Source {
            newest: checkpoint(&signing, 1, 60),
            of_epoch: None,
            asked: Mutex::default(),
        };
        let mut seven = committee.clone();
        seven.extend(keys().1.into_iter().take(3));
        assert!(fetch(at(60, 2), &source, &seven, None).unwrap().is_empty());
    }

    #[test]
    fn an_epochs_final_checkpoint_from_another_epoch_is_refused_by_label() {
        let (signing, committee) = keys();
        let source = Source {
            newest: checkpoint(&signing, 3, 200),
            of_epoch: Some(checkpoint(&signing, 2, 150)),
            asked: Mutex::default(),
        };
        let err = fetch(at(40, 1), &source, &committee, None).unwrap_err();
        assert!(err.to_string().contains("sent one of epoch 2"), "{err}");
    }

    #[test]
    fn in_the_same_epoch_it_never_asks_for_an_epochs_final_checkpoint() {
        let (signing, committee) = keys();
        let source = Source {
            newest: checkpoint(&signing, 1, 50),
            of_epoch: None,
            asked: Mutex::default(),
        };
        assert!(
            fetch(at(50, 1), &source, &committee, None)
                .unwrap()
                .is_empty()
        );
        assert!(source.asked.lock().expect("lock").is_empty());
    }

    /// Serves a chain of `len` linked blocks and counts the requests.
    struct Chained {
        blocks: Vec<Block>,
        newest: Checkpoint,
        requests: std::sync::atomic::AtomicUsize,
        batches: bool,
    }

    impl CheckpointSource for Chained {
        fn checkpoint(&self) -> Result<Option<Checkpoint>> {
            Ok(Some(self.newest.clone()))
        }
        fn checkpoint_of(&self, _: u64) -> Result<Option<Checkpoint>> {
            Ok(None)
        }
        fn block(&self, height: u64) -> Result<Block> {
            self.requests
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(self.blocks[usize::try_from(height - 1).unwrap()].clone())
        }
        fn blocks(&self, from: u64, to: u64) -> Result<Vec<Block>> {
            if !self.batches {
                return (from..=to).map(|h| self.block(h)).collect();
            }
            self.requests
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let end = usize::try_from(to).unwrap().min(self.blocks.len());
            Ok(self.blocks[usize::try_from(from - 1).unwrap()..end].to_vec())
        }
    }

    /// `len` blocks linked from `[0; 32]`, and a checkpoint on the last.
    fn chained(len: u64, batches: bool) -> (Chained, Vec<VerifyingKey>) {
        let (signing, committee) = keys();
        let mut parent = [0; 32];
        let mut blocks = Vec::new();
        for h in 1..=len {
            let block = Block::new(
                crate::core::BlockHeader {
                    version: 0,
                    prev_hash: parent,
                    state_root: [0; 32],
                    timestamp: h,
                    nonce: 0,
                    difficulty_target: [0xff; 32],
                    tx_root: [0; 32],
                },
                Vec::new(),
            );
            parent = block.header.id();
            blocks.push(block);
        }
        let mut newest = checkpoint(&signing, 1, len);
        newest.block = parent;
        // A checkpoint signs (epoch, height, block): re-sign over the real id.
        newest.signatures = (0..3u16)
            .map(|v| {
                let a =
                    Attestation::sign(&signing[usize::from(v)], v, &TAG, 1, len, parent).unwrap();
                (v, a.signature)
            })
            .collect();
        let source = Chained {
            blocks,
            newest,
            requests: std::sync::atomic::AtomicUsize::new(0),
            batches,
        };
        (source, committee)
    }

    /// Node at height 10 on the chain of `chained`, its tip the real block 10.
    fn at_ten(source: &Chained) -> Position {
        Position {
            tip: source.blocks[9].header.id(),
            ..at(10, 1)
        }
    }

    fn trusted_at(source: &Chained, height: u64) -> TrustedBlock {
        let block = &source.blocks[usize::try_from(height - 1).unwrap()];
        TrustedBlock {
            height,
            id: block.header.id(),
        }
    }

    #[test]
    fn a_stale_node_links_its_gap_to_the_trusted_block_without_a_committee() {
        // Why: the committee here signs nothing the source holds, so a
        // committee check would refuse. Passing shows the gap is linked by
        // hash to the trusted block alone (the stale-node rule).
        let (source, _) = chained(40, true);
        let got = fetch_anchored(
            at_ten(&source),
            &source,
            &[],
            None,
            Anchor::Stale(trusted_at(&source, 25)),
        )
        .unwrap();
        assert_eq!(got.len(), 15);
        assert_eq!(got.last().unwrap().header.id(), trusted_at(&source, 25).id);
    }

    #[test]
    fn a_chain_that_is_not_the_trusted_one_stops_the_catch_up_with_nothing_returned() {
        // Why: a long-range forger can serve a chain of its own. Its blocks
        // cannot hash to the trusted id, so the batch must be refused whole.
        let (source, committee) = chained(40, true);
        let wrong = TrustedBlock {
            height: 25,
            id: [0xEE; 32],
        };
        for anchor in [Anchor::Stale(wrong), Anchor::Checked(wrong)] {
            let err =
                fetch_anchored(at_ten(&source), &source, &committee, None, anchor).unwrap_err();
            assert!(
                matches!(&err, NodeError::Decode(m) if m.contains("trusted")),
                "{anchor:?}: {err}"
            );
        }
    }

    #[test]
    fn an_ordinary_batch_that_reaches_the_trusted_block_must_carry_its_id() {
        let (source, committee) = chained(40, true);
        let right = Anchor::Checked(trusted_at(&source, 25));
        let got = fetch_anchored(at_ten(&source), &source, &committee, None, right).unwrap();
        assert_eq!(got.len(), 30);
        // A batch stopping short of the trusted height checks nothing.
        let short = Anchor::Checked(TrustedBlock {
            height: 45,
            id: [0xEE; 32],
        });
        assert!(fetch_anchored(at_ten(&source), &source, &committee, None, short).is_ok());
    }

    #[test]
    fn a_stale_node_past_its_trusted_block_follows_the_committee_again() {
        // Past the anchor the normal rule applies, so the same empty committee
        // now fails the quorum check.
        let (source, _) = chained(40, true);
        let past = Anchor::Stale(trusted_at(&source, 5));
        assert!(fetch_anchored(at_ten(&source), &source, &[], None, past).is_err());
    }

    #[test]
    fn a_stale_node_whose_tip_is_the_trusted_block_is_past_the_anchor() {
        // Boundary: from.height == trusted.height. The gap is empty, so the
        // anchor is met and the committee rule applies (same empty committee
        // fails the quorum).
        let (source, _) = chained(40, true);
        let at = Anchor::Stale(trusted_at(&source, 10));
        assert!(fetch_anchored(at_ten(&source), &source, &[], None, at).is_err());
    }

    #[test]
    fn a_checked_anchor_at_or_below_the_tip_does_not_re_check_the_chain() {
        // The start-up rule checks a block at or below the tip; a batch past it
        // must not re-check a trusted id it cannot see, so the committee path runs.
        let (source, committee) = chained(40, true);
        let below = Anchor::Checked(TrustedBlock {
            height: 5,
            id: [0xEE; 32],
        });
        assert!(fetch_anchored(at_ten(&source), &source, &committee, None, below).is_ok());
    }

    #[test]
    fn a_link_across_more_than_the_cap_is_refused_before_any_request() {
        // Why: the cap must trip before the source is asked for a single block.
        // The chain here has 40 blocks, so any request would fail differently.
        let (source, _) = chained(40, true);
        let far = Anchor::Stale(TrustedBlock {
            height: 10 + MAX_LINKED_GAP + 1,
            id: [1; 32],
        });
        let err = fetch_anchored(at_ten(&source), &source, &[], None, far).unwrap_err();
        assert!(err.to_string().contains("cap"), "{err}");
        assert_eq!(source.requests.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn the_cap_is_inclusive_and_names_the_gap() {
        assert!(check_gap(0, MAX_LINKED_GAP).is_ok());
        assert!(check_gap(0, MAX_LINKED_GAP + 1).is_err());
        assert!(
            check_gap(100, 50).is_ok(),
            "a trusted block below the start saturates to 0"
        );
    }

    #[test]
    fn the_byte_cap_aborts_the_link_and_a_gap_within_it_links() {
        // Why: the byte total is checked as responses arrive, so a gap over the
        // limit is refused with the error naming bytes, not linked and then checked.
        let (source, _) = chained(40, true);
        let id = trusted_at(&source, 25).id;
        let err = fetch_linked(at_ten(&source), 25, id, &source, "trusted", Some(1)).unwrap_err();
        assert!(err.to_string().contains("bytes"), "{err}");
        let got = fetch_linked(
            at_ten(&source),
            25,
            id,
            &source,
            "trusted",
            Some(MAX_LINKED_GAP_BYTES),
        )
        .unwrap();
        assert_eq!(got.len(), 15);
    }

    #[test]
    fn many_blocks_arrive_in_a_few_requests_and_in_order() {
        let (source, committee) = chained(100, true);
        let got = fetch(at(0, 1), &source, &committee, None).unwrap();
        assert_eq!(got.len(), 100);
        let requests = source.requests.load(std::sync::atomic::Ordering::SeqCst);
        // 100 blocks at BLOCKS_PER_REQUEST each; one request per block is the
        // latency-bound path that made a peer hours behind take an hour.
        assert_eq!(requests, 100usize.div_ceil(BLOCKS_PER_REQUEST as usize));
    }

    #[test]
    fn a_source_without_batches_still_catches_up_block_by_block() {
        let (source, committee) = chained(40, false);
        let got = fetch(at(0, 1), &source, &committee, None).unwrap();
        assert_eq!(got.len(), 40);
        assert_eq!(
            source.requests.load(std::sync::atomic::Ordering::SeqCst),
            40
        );
    }

    #[test]
    fn a_short_batch_is_an_error_not_a_silent_gap() {
        let (mut source, committee) = chained(40, true);
        source.blocks.truncate(39); // claims 40, serves 39
        let err = fetch(at(0, 1), &source, &committee, None);
        assert!(err.is_err());
    }

    #[test]
    fn a_peer_without_the_epoch_or_short_of_the_boundary_is_an_error_not_silence() {
        let (signing, committee) = keys();
        let forgotten = Source {
            newest: checkpoint(&signing, 9, 900),
            of_epoch: None,
            asked: Mutex::default(),
        };
        let err = fetch(at(40, 1), &forgotten, &committee, None).unwrap_err();
        assert!(err.to_string().contains("snapshot"), "{err}");
        let short = Source {
            newest: checkpoint(&signing, 2, 90),
            of_epoch: Some(checkpoint(&signing, 1, 59)),
            asked: Mutex::default(),
        };
        let err = fetch(at(59, 1), &short, &committee, None).unwrap_err();
        assert!(
            err.to_string().contains("stuck at the end of epoch 1"),
            "{err}"
        );
    }
}
