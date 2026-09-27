//! A committed sub-DAG becomes exactly one block.
//!
//! Every node runs this on the same sub-DAG and the same parent state, so every
//! node builds the same block: the block is *derived* from the order, never
//! proposed. That is why a DAG-BFT node never imports a block from gossip — a
//! block it did not derive is a claim, and the certificates are the proof.
//!
//! The header's fields keep their proof-of-work layout (spec/ §2) and change
//! meaning under this mode:
//!
//! - `nonce` packs the anchor: [`seal`] = epoch in the top 24 bits, round in
//!   the low 40. It is how a restarted node knows which anchors it has already
//!   turned into blocks, and how a reader finds a block's certificate.
//! - `timestamp` is the anchor's certified proposal time, in seconds, clamped
//!   to never run behind the parent. It is read from a signed vertex, not from
//!   anyone's clock, so it is the same on every node.
//! - `difficulty_target` is whatever the retarget rule yields at the chain's
//!   unlimited floor; it carries no work and the chain verifies none.

use std::collections::BTreeSet;

use maya_dag_bft::SubDag;

use crate::consensus::Chain;
use crate::core::{Block, Transaction};
use crate::error::Result;
use crate::state::context::BlockContext;

/// Bits of the nonce that hold the anchor round.
const ROUND_BITS: u32 = 40;

/// Packs an anchor's epoch and round into a header nonce.
#[must_use]
pub const fn seal(epoch: u64, round: u64) -> u64 {
    (epoch << ROUND_BITS) | (round & ((1 << ROUND_BITS) - 1))
}

/// The `(epoch, round)` a header nonce names.
#[must_use]
pub const fn unseal(nonce: u64) -> (u64, u64) {
    (nonce >> ROUND_BITS, nonce & ((1 << ROUND_BITS) - 1))
}

/// Decodes every payload in commit order and drops what is not a transaction
/// or repeats one already seen. Validators propose from their own mempools,
/// so the same transaction arriving in two vertices is the normal case.
fn ordered_transactions(sub_dag: &SubDag) -> Vec<Transaction> {
    let mut seen = BTreeSet::new();
    sub_dag
        .certificates
        .iter()
        .flat_map(|c| c.vertex.batch.iter())
        .filter_map(|bytes| Transaction::from_bytes(bytes).ok())
        .filter(|tx| seen.insert(tx.txid()))
        .collect()
}

/// Builds the block `sub_dag` orders on top of `chain`'s tip.
///
/// Transactions that do not execute in order are dropped
/// ([`crate::state::StateDB::select_applicable`]). If the survivors still
/// fail at the end-of-block passes, the block is built empty: an anchor must
/// always produce a block, or two nodes that disagreed about one transaction
/// would disagree about the height of everything after it.
///
/// # Errors
///
/// Only if even the empty block cannot be built — the tip is missing from the
/// index or the state cannot be read — which is a local fault, not a
/// consensus outcome.
pub fn build_block(chain: &Chain, sub_dag: &SubDag) -> Result<Block> {
    let anchor = &sub_dag.anchor.vertex;
    let parent = chain.get(&chain.tip()).map(|r| r.header.timestamp);
    let timestamp = (anchor.timestamp_ms / 1_000).max(parent.unwrap_or(0));
    let context = BlockContext::at_height(chain.height() + 1);
    let target = chain.next_target(&chain.tip())?;
    let kept = chain
        .state()
        .select_applicable(ordered_transactions(sub_dag), context, target);
    let mut block = match chain.candidate_block(timestamp, kept) {
        Ok(block) => block,
        Err(_) => chain.candidate_block(timestamp, Vec::new())?,
    };
    block.header.nonce = seal(anchor.epoch, anchor.round);
    Ok(block)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seal_round_trips_epoch_and_round() {
        for (epoch, round) in [(0, 0), (0, 2), (7, 123_456), ((1 << 24) - 1, (1 << 40) - 1)] {
            assert_eq!(unseal(seal(epoch, round)), (epoch, round));
        }
    }
}
