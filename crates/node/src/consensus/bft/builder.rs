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
//! - `timestamp` is the **median** certified proposal time of the sub-DAG's
//!   vertices, in seconds, clamped to never run behind the parent. Read from
//!   signed vertices, not from anyone's clock, so it is the same on every
//!   node; a median over vertices from at least 2f + 1 authors, so no f
//!   faulty clocks can move it. (The first version used the anchor's own
//!   time: the clock-drift rehearsal showed one validator ten minutes fast
//!   leaving the chain's clock ten minutes ahead for good.)
//! - `difficulty_target` is whatever the retarget rule yields at the chain's
//!   unlimited floor; it carries no work and the chain verifies none.

use std::collections::BTreeSet;

use maya_dag_bft::SubDag;
use maya_fee_market::limits::MAX_BLOCK_BYTES;

use crate::consensus::Chain;
use crate::core::block::decode_cut;
use crate::core::{Block, Transaction};
use crate::error::Result;

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

/// Raw bytes of batch payloads one block may decode: the block cap, since no
/// block can carry more. Decoding past it is work no block can use.
pub const DECODE_BUDGET: u64 = MAX_BLOCK_BYTES;

/// Decodes the distinct transactions among `payloads`, in order, and stops
/// before the first new payload that would take the decoded bytes past
/// `budget`. A payload identical to an earlier one is skipped without a
/// decode, so repeats cost nothing. Every node walks the same order, so every
/// node decodes the same prefix. Returns the transactions and the bytes charged.
pub fn decode_within<'a>(
    payloads: impl IntoIterator<Item = &'a [u8]>,
    budget: u64,
) -> (Vec<Transaction>, u64) {
    let mut raw_seen = BTreeSet::new();
    let distinct: Vec<&[u8]> = payloads
        .into_iter()
        .filter(|payload| raw_seen.insert(*payload))
        .collect();
    let cut = decode_cut(distinct.iter().map(|payload| payload.len()), budget);
    let charged = distinct[..cut].iter().map(|p| p.len() as u64).sum::<u64>();
    let mut ids = BTreeSet::new();
    let txs: Vec<Transaction> = distinct[..cut]
        .iter()
        .filter_map(|payload| Transaction::from_bytes(payload).ok())
        .filter(|tx| ids.insert(tx.txid()))
        .collect();
    (txs, charged)
}

/// Every batch payload of `sub_dag`, in commit order.
pub fn batch_payloads(sub_dag: &SubDag) -> impl Iterator<Item = &[u8]> {
    sub_dag
        .certificates
        .iter()
        .flat_map(|c| c.vertex.batch.iter().map(Vec::as_slice))
}

/// Decodes the sub-DAG's transactions in commit order, within [`DECODE_BUDGET`].
/// Validators propose from their own mempools, so the same transaction arriving
/// in two vertices is the normal case. Those past the budget are not decoded
/// and are not in the block; they stay in their proposers' mempools.
fn ordered_transactions(sub_dag: &SubDag) -> Vec<Transaction> {
    decode_within(batch_payloads(sub_dag), DECODE_BUDGET).0
}

/// The median proposal time of the sub-DAG's certified vertices, ignoring
/// genesis vertices (time 0). The upper median of an even count, so the rule
/// has one answer. Falls back to the anchor's own time for a sub-DAG with no
/// timed vertex, which only the first rounds can produce.
fn median_time_ms(sub_dag: &SubDag) -> u64 {
    let mut times: Vec<u64> = sub_dag
        .certificates
        .iter()
        .map(|c| c.vertex.timestamp_ms)
        .filter(|t| *t > 0)
        .collect();
    if times.is_empty() {
        return sub_dag.anchor.vertex.timestamp_ms;
    }
    times.sort_unstable();
    times[times.len() / 2]
}

/// Builds the block `sub_dag` orders on top of `chain`'s tip.
///
/// Transactions that do not execute in order are dropped
/// ([`crate::state::StateDB::select_applicable`]). Ones past the block cap
/// (ADR-072) are not dropped: they are left out, and stay with their proposer
/// for a later block. If the survivors still
/// fail at the end-of-block passes, the block is built empty: an anchor must
/// always produce a block, or two nodes that disagreed about one transaction
/// would disagree about the height of everything after it.
///
/// # Errors
///
/// Only if even the empty block cannot be built — the tip is missing from the
/// index, the state cannot be read, or it is bound to no chain — which is a
/// local fault, not a consensus outcome.
pub fn build_block(chain: &Chain, sub_dag: &SubDag) -> Result<Block> {
    let anchor = &sub_dag.anchor.vertex;
    let parent = chain.get(&chain.tip()).map(|r| r.header.timestamp);
    let timestamp = (median_time_ms(sub_dag) / 1_000).max(parent.unwrap_or(0));
    let context = chain.context_at(chain.height() + 1);
    let target = chain.next_target(&chain.tip())?;
    let kept = chain
        .state()
        .select_applicable(ordered_transactions(sub_dag), context, target)?;
    let nonce = seal(anchor.epoch, anchor.round);
    match chain.candidate_block_sealed(timestamp, kept, nonce) {
        Ok(block) => Ok(block),
        Err(_) => chain.candidate_block_sealed(timestamp, Vec::new(), nonce),
    }
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

    use crate::core::transaction::TxOutput;
    use primitive_types::U256;

    const MIB: usize = 1 << 20;

    fn payload(nonce: u64) -> Vec<u8> {
        let output = TxOutput {
            amount: U256::from(1_u64),
            recipient: [7; 32],
        };
        Transaction::new(vec![], vec![output], nonce).to_bytes()
    }

    #[test]
    fn a_walk_across_many_stalled_rounds_decodes_within_the_budget() {
        // 100 distinct 1 MiB payloads: a commit that spans stalled rounds.
        let junk: Vec<Vec<u8>> = (0..100_u8).map(|i| vec![i; MIB]).collect();
        let first = decode_within(junk.iter().map(Vec::as_slice), DECODE_BUDGET);
        let again = decode_within(junk.iter().map(Vec::as_slice), DECODE_BUDGET);
        assert_eq!(
            first.1,
            5 * MIB as u64,
            "the walk stops before the sixth payload"
        );
        assert!(first.1 <= DECODE_BUDGET);
        assert_eq!(first.0.len(), again.0.len(), "two walks of one order agree");
        assert_eq!(first.1, again.1);
    }

    #[test]
    fn a_repeated_payload_is_decoded_and_charged_once() {
        let (a, b) = (payload(1), payload(2));
        let order = [a.as_slice(), a.as_slice(), a.as_slice(), b.as_slice()];
        let (txs, charged) = decode_within(order, (a.len() + b.len()) as u64);
        let ids: Vec<_> = txs.iter().map(Transaction::txid).collect();
        assert_eq!(ids.len(), 2);
        assert_eq!(charged, (a.len() + b.len()) as u64);
    }

    #[test]
    fn transactions_past_the_budget_are_left_for_a_later_walk() {
        let (p1, p2, p3) = (payload(1), payload(2), payload(3));
        let budget = (p1.len() + p2.len()) as u64;
        let (first, _) = decode_within([p1.as_slice(), p2.as_slice(), p3.as_slice()], budget);
        assert_eq!(first.len(), 2);
        // The next block's walk sees the leftover, and only the leftover is new.
        let (second, _) = decode_within([p3.as_slice(), p3.as_slice()], DECODE_BUDGET);
        assert_eq!(second.len(), 1);
        assert_eq!(
            second[0].txid(),
            Transaction::from_bytes(&p3).unwrap().txid()
        );
    }
}
