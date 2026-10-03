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
use crate::crypto::keys::VerifyingKey;
use crate::error::{NodeError, Result};

use super::attest::Checkpoint;

/// Where a node that fell behind gets a checkpoint and the blocks up to it.
/// Every answer is untrusted; [`catch_up`] checks all of it.
pub trait CheckpointSource {
    /// The source's newest checkpoint, if it holds one.
    ///
    /// # Errors
    ///
    /// Any failure reaching the source.
    fn checkpoint(&self) -> Result<Option<Checkpoint>>;

    /// The active-chain block at `height`.
    ///
    /// # Errors
    ///
    /// Any failure reaching the source.
    fn block(&self, height: u64) -> Result<Block>;
}

/// Imports blocks from `source` up to its newest checkpoint, returning how
/// many were imported (zero when already there).
///
/// `committee` is the committee this node already trusts — the genesis or
/// staking committee in its own state. A checkpoint signed by another
/// committee is refused: crossing a membership change needs that change's
/// own evidence, which this does not yet fetch.
///
/// # Errors
///
/// [`NodeError::Decode`] for a checkpoint without a quorum of `committee`,
/// a block that does not chain to it, or one the chain does not extend with;
/// errors from `source` or from executing a block are returned as they come.
/// Blocks imported before an error stay: each one was re-executed and
/// chains to an attested block, so it is valid where it is.
pub fn catch_up(
    chain: &mut Chain,
    source: &impl CheckpointSource,
    committee: &[VerifyingKey],
) -> Result<u64> {
    let Some(checkpoint) = source.checkpoint()? else {
        return Ok(0);
    };
    let tag = ChainTag::from_genesis(chain.genesis());
    checkpoint.verify(&tag, committee)?;
    let from = chain.height();
    if checkpoint.height <= from {
        return Ok(0);
    }
    let blocks = fetch_chained(chain, source, &checkpoint)?;
    for (offset, block) in blocks.into_iter().enumerate() {
        let height = from + 1 + offset as u64;
        match chain.insert_block(block)? {
            InsertOutcome::Extended { .. } => {}
            other => {
                return Err(NodeError::Decode(format!(
                    "catch-up: block {height} did not extend the tip: {other:?}"
                )));
            }
        }
    }
    Ok(checkpoint.height - from)
}

/// The blocks after this node's tip up to the checkpoint, checked to form
/// one hash chain from the tip to the attested block before any executes.
fn fetch_chained(
    chain: &Chain,
    source: &impl CheckpointSource,
    checkpoint: &Checkpoint,
) -> Result<Vec<Block>> {
    let mut parent = chain.tip();
    let mut blocks = Vec::new();
    for height in chain.height() + 1..=checkpoint.height {
        let block = source.block(height)?;
        if block.header.prev_hash != parent {
            return Err(NodeError::Decode(format!(
                "catch-up: block {height} does not extend the one before it"
            )));
        }
        parent = block.header.id();
        blocks.push(block);
    }
    if parent != checkpoint.block {
        return Err(NodeError::Decode(
            "catch-up: the blocks do not lead to the attested block".to_string(),
        ));
    }
    Ok(blocks)
}
