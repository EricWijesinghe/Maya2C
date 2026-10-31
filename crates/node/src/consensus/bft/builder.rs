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
use crate::core::block::decode_plan;
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

/// Raw bytes of batch payloads one block may decode (CON-13): 3/2 of the block
/// cap. With f < n/3 junk authors, honest authors' first-turn credit still
/// covers one full cap (ADR-072, "Fair share").
pub const DECODE_BUDGET: u64 = MAX_BLOCK_BYTES * 3 / 2;

/// Decodes the sub-DAG's transactions within [`DECODE_BUDGET`], shared fairly
/// among authors (CON-13), and returns them in commit order with the bytes
/// charged. A payload identical to an admitted one is not decoded again.
/// Payloads not admitted are not in the block; they stay with their proposers.
pub fn decode_fair(sub_dag: &SubDag, committee_size: usize) -> (Vec<Transaction>, u64) {
    let entries: Vec<(u16, &[u8])> = sub_dag
        .certificates
        .iter()
        .flat_map(|c| {
            c.vertex
                .batch
                .iter()
                .map(move |p| (c.vertex.author, p.as_slice()))
        })
        .collect();
    decode_fair_payloads(&entries, committee_size, DECODE_BUDGET)
}

/// [`decode_fair`] over `(author, payload)` pairs in commit order, with the
/// budget explicit so tests can use small ones.
pub fn decode_fair_payloads(
    entries: &[(u16, &[u8])],
    committee_size: usize,
    budget: u64,
) -> (Vec<Transaction>, u64) {
    let plan: Vec<(u16, usize, &[u8])> = entries
        .iter()
        .map(|(author, payload)| (*author, payload.len(), *payload))
        .collect();
    let (admitted, charged) = decode_plan(&plan, committee_size, budget);
    let mut ids = BTreeSet::new();
    let txs = admitted
        .iter()
        .filter_map(|&index| Transaction::from_bytes(entries[index].1).ok())
        .filter(|tx| ids.insert(tx.txid()))
        .collect();
    (txs, charged)
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
pub fn build_block(chain: &Chain, sub_dag: &SubDag, committee_size: usize) -> Result<Block> {
    let anchor = &sub_dag.anchor.vertex;
    let parent = chain.get(&chain.tip()).map(|r| r.header.timestamp);
    let timestamp = (median_time_ms(sub_dag) / 1_000).max(parent.unwrap_or(0));
    let context = chain.context_at(chain.height() + 1);
    let target = chain.next_target(&chain.tip())?;
    let kept =
        chain
            .state()
            .select_applicable(decode_fair(sub_dag, committee_size).0, context, target)?;
    let nonce = seal(anchor.epoch, anchor.round);
    match chain.candidate_block_sealed(timestamp, kept, nonce) {
        Ok(block) => Ok(block),
        Err(_) => chain.candidate_block_sealed(timestamp, Vec::new(), nonce),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::block::{decode_cut, decode_plan};
    use crate::core::transaction::TxOutput;
    use primitive_types::U256;

    const MIB: usize = 1 << 20;

    #[test]
    fn the_seal_round_trips_epoch_and_round() {
        for (epoch, round) in [(0, 0), (0, 2), (7, 123_456), ((1 << 24) - 1, (1 << 40) - 1)] {
            assert_eq!(unseal(seal(epoch, round)), (epoch, round));
        }
    }

    /// A transfer payload with `outputs` outputs, so tests can use unequal sizes.
    fn tx_bytes(nonce: u64, outputs: usize) -> Vec<u8> {
        let outputs = (0..outputs)
            .map(|_| TxOutput {
                amount: U256::from(1_u64),
                recipient: [7; 32],
            })
            .collect();
        Transaction::new(vec![], outputs, nonce).to_bytes()
    }

    /// Distinct junk: undecodable, so only its length matters to the walk.
    fn junk(tag: u8, len: usize) -> Vec<u8> {
        vec![tag; len]
    }

    fn ids(txs: &[Transaction]) -> Vec<[u8; 32]> {
        txs.iter().map(Transaction::txid).collect()
    }

    /// CON-13 vector 1: a low-index author with 10 x 1 MiB junk, and authors 1..3
    /// with 0.5 MiB each. Budget 5 MiB, four authors: quantum 1.25 MiB.
    #[test]
    fn a_junk_heavy_low_index_author_cannot_starve_the_others() {
        let mut order: Vec<(u16, usize, u64)> =
            (0..10_u8).map(|i| (0, MIB, u64::from(i))).collect();
        order.extend((1..=3_u16).map(|a| (a, MIB / 2, 100 + u64::from(a))));
        let (admitted, charged) = decode_plan(&order, 4, 5 * MIB as u64);
        // Author 0 gets three 1 MiB payloads; authors 1..3 each get their 0.5 MiB.
        assert_eq!(admitted, vec![0, 1, 2, 10, 11, 12]);
        assert_eq!(charged, (4 * MIB + MIB / 2) as u64);
        assert!(charged <= 5 * MIB as u64);
    }

    /// The same stalled order through the builder's byte path: the transfers
    /// of authors 1..3 are decoded, behind a junk-heavy author 0.
    #[test]
    fn a_junk_heavy_author_does_not_hide_transfers_from_the_walk() {
        let junk_bytes: Vec<Vec<u8>> = (0..100_u8).map(|i| junk(i, MIB)).collect();
        let transfers: Vec<Vec<u8>> = (1..=3_u16).map(|a| tx_bytes(u64::from(a), 16)).collect();
        let mut entries: Vec<(u16, &[u8])> = junk_bytes.iter().map(|j| (0, j.as_slice())).collect();
        entries.extend(
            transfers
                .iter()
                .enumerate()
                .map(|(i, t)| (i as u16 + 1, t.as_slice())),
        );
        let (decoded, charged) = decode_fair_payloads(&entries, 4, DECODE_BUDGET);
        assert_eq!(
            decoded.len(),
            3,
            "each of authors 1..3 contributes its transfer"
        );
        assert!(charged <= DECODE_BUDGET);
    }

    /// A single author is the old cut: a prefix that stops at the first misfit.
    #[test]
    fn one_author_matches_the_old_prefix_cut() {
        let txs: Vec<Vec<u8>> = [1_usize, 40, 3, 200, 2]
            .iter()
            .enumerate()
            .map(|(n, outputs)| tx_bytes(n as u64, *outputs))
            .collect();
        let entries: Vec<(u16, &[u8])> = txs.iter().map(|t| (7, t.as_slice())).collect();
        let budget = (txs[0].len() + txs[1].len() + txs[2].len() + 10) as u64;
        let lens: Vec<usize> = txs.iter().map(Vec::len).collect();
        let old = decode_cut(lens, budget);
        let (decoded, _) = decode_fair_payloads(&entries, 16, budget);
        assert_eq!(decoded.len(), old);
        assert_eq!(
            ids(&decoded),
            ids(&txs[..old]
                .iter()
                .map(|t| Transaction::from_bytes(t).unwrap())
                .collect::<Vec<_>>())
        );
    }

    /// A transaction repeated across authors is charged and decoded once, at the
    /// admitted copy's position.
    #[test]
    fn a_repeat_across_authors_is_charged_and_decoded_once() {
        let shared = tx_bytes(1, 4);
        let other = tx_bytes(2, 4);
        let entries: Vec<(u16, &[u8])> = vec![
            (0, shared.as_slice()),
            (1, shared.as_slice()),
            (1, other.as_slice()),
        ];
        let (decoded, charged) = decode_fair_payloads(&entries, 2, DECODE_BUDGET);
        assert_eq!(decoded.len(), 2);
        assert_eq!(charged, (shared.len() + other.len()) as u64);
    }

    /// Author 2 has nothing: authors 0, 1 and 3 share the whole budget, so the
    /// block is not capped at 3/4. Author 0's 1 MiB leaves 7 MiB; author 0's
    /// 7.5 MiB next payload finishes only author 0, and author 1 uses the rest.
    #[test]
    fn an_absent_authors_share_is_redistributed() {
        let a0_small = junk(1, MIB);
        let a0_big = junk(2, MIB * 15 / 2);
        let a1: Vec<Vec<u8>> = (0..8_u8).map(|i| junk(10 + i, MIB)).collect();
        let mut entries: Vec<(u16, &[u8])> = vec![(0, a0_small.as_slice()), (0, a0_big.as_slice())];
        entries.extend(a1.iter().map(|j| (1, j.as_slice())));
        let budget = 8 * MIB as u64;
        let (_, charged) = decode_fair_payloads(&entries, 4, budget);
        assert_eq!(
            charged, budget,
            "the whole budget is used while demand exists"
        );
        assert!(charged * 4 > budget * 3);
    }

    /// A payload larger than what is left finishes only its own author; the
    /// others continue, and the finished author's later payloads are not touched.
    #[test]
    fn a_misfit_finishes_only_its_author() {
        let a0_big = junk(1, MIB * 5 / 2);
        let a0_tail = junk(2, MIB / 10);
        let a1: Vec<Vec<u8>> = vec![junk(3, MIB / 2), junk(4, MIB / 2)];
        let entries: Vec<(u16, &[u8])> = vec![
            (0, a0_big.as_slice()),
            (0, a0_tail.as_slice()),
            (1, a1[0].as_slice()),
            (1, a1[1].as_slice()),
        ];
        let plan: Vec<(u16, usize, &[u8])> =
            entries.iter().map(|(a, p)| (*a, p.len(), *p)).collect();
        let (admitted, charged) = decode_plan(&plan, 2, 3 * MIB as u64);
        assert_eq!(
            admitted,
            vec![2, 3],
            "author 0's tail is not admitted past its misfit"
        );
        assert_eq!(charged, MIB as u64);
    }

    /// Admitted transactions come out in commit order, across authors.
    #[test]
    fn admitted_transactions_come_out_in_commit_order() {
        let a = tx_bytes(10, 2);
        let b = tx_bytes(20, 2);
        let c = tx_bytes(30, 2);
        let entries: Vec<(u16, &[u8])> =
            vec![(2, a.as_slice()), (0, b.as_slice()), (1, c.as_slice())];
        let (decoded, _) = decode_fair_payloads(&entries, 3, DECODE_BUDGET);
        let expected: Vec<[u8; 32]> = [&a, &b, &c]
            .iter()
            .map(|t| Transaction::from_bytes(t).unwrap().txid())
            .collect();
        assert_eq!(ids(&decoded), expected);
    }
}
