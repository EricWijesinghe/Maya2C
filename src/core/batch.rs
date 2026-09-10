//! Batch identifiers, and the shard access set of a transaction.
//!
//! # Status
//!
//! **Research branch. No consensus path reaches this module.** It is the
//! node-side half of [`maya_blockgraph`], which carries the bounds and the
//! scheduler and the reasons they live in a crate of their own. The network
//! worker that would disseminate batches and the header field that would
//! reference them are not written. See `docs/blockgraph.md`.
//!
//! # Why the derivation is here and not there
//!
//! `maya-blockgraph` has no dependencies, so Kani can compile it. That rules
//! out BLAKE3 and therefore rules out computing a digest. The same boundary
//! `maya-dex` and `maya-lattice-pow` live behind.

use blake3::Hasher;
use maya_blockgraph::{Access, Batch, BatchId, GraphError, ShardId, shard_of};

use crate::core::transaction::Transaction;
use crate::state::account::Address;

/// BLAKE3 derive-key domain for batch identifiers.
///
/// Consensus, in the same way the state-layer domains in
/// [`crate::state::proof`] are. Two subsystems hashing the same bytes under one
/// domain would produce colliding identifiers for unrelated objects.
pub const BATCH_ID_DOMAIN: &str = "maya batch identifier v1";

/// The identifier of a batch: a digest over its transactions, length-prefixed.
///
/// # Why every transaction is length-prefixed
///
/// Without it, a batch of `[ab, c]` and one of `[a, bc]` hash the same bytes
/// and collide. They are different batches — they execute differently — so an
/// identifier that could not tell them apart would let a peer answer a fetch
/// for one with the other, and the fetching node could not detect it.
///
/// The count is prefixed for the same reason at one level up.
#[must_use]
pub fn batch_id(batch: &Batch) -> BatchId {
    let mut hasher = Hasher::new_derive_key(BATCH_ID_DOMAIN);
    hasher.update(&(batch.len() as u64).to_le_bytes());
    for transaction in batch.transactions() {
        hasher.update(&(transaction.len() as u64).to_le_bytes());
        hasher.update(transaction);
    }
    *hasher.finalize().as_bytes()
}

/// The shards a set of addresses touches.
///
/// # Errors
///
/// [`GraphError::EmptyAccessSet`] if `addresses` is empty. A transaction that
/// touched no account would be scheduled as conflict-free with everything,
/// which is the one input that makes the scheduler produce a wrong answer.
pub fn access_for_addresses(addresses: &[Address]) -> Result<Access, GraphError> {
    let shards: Vec<ShardId> = addresses.iter().map(shard_of).collect();
    Access::from_shards(&shards)
}

/// The shard access set of a transaction.
///
/// # Why this must over-approximate, never under-approximate
///
/// The scheduler is handed this set and trusts it. A set that is too *wide*
/// costs parallelism: the transaction serialises against more of its
/// neighbours than it needed to, and the block takes longer to execute. A set
/// that is too *narrow* is a consensus failure: two transactions that actually
/// contend get placed in one wave, execute concurrently, and produce a state
/// root that depends on which thread won.
///
/// The two errors are not symmetric, so this function resolves every doubt
/// towards the wider set. Anything added to [`crate::core::payload`] that
/// touches state not named by an input or an output has to be reflected here,
/// and the failure mode of forgetting is silent.
///
/// # Errors
///
/// [`GraphError::EmptyAccessSet`] for a transaction naming no address at all,
/// which the transaction rules already reject before this is reached.
pub fn access_for_transaction(transaction: &Transaction) -> Result<Access, GraphError> {
    let mut addresses: Vec<Address> = Vec::new();

    addresses.push(transaction.sender());
    for output in &transaction.outputs {
        addresses.push(output.recipient);
    }

    access_for_addresses(&addresses)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch_of(transactions: Vec<Vec<u8>>) -> Batch {
        Batch::new(transactions).expect("valid batch")
    }

    #[test]
    fn the_same_batch_always_has_the_same_identifier() {
        let batch = batch_of(vec![vec![1, 2, 3], vec![4, 5]]);
        assert_eq!(batch_id(&batch), batch_id(&batch));
    }

    #[test]
    fn reordering_a_batch_changes_its_identifier() {
        // Order is part of what a batch is: the two execute differently.
        let forward = batch_of(vec![vec![1], vec![2]]);
        let backward = batch_of(vec![vec![2], vec![1]]);
        assert_ne!(batch_id(&forward), batch_id(&backward));
    }

    #[test]
    fn a_different_split_of_the_same_bytes_has_a_different_identifier() {
        // The case length-prefixing exists for: without it these collide.
        let left = batch_of(vec![vec![0xAA, 0xBB], vec![0xCC]]);
        let right = batch_of(vec![vec![0xAA], vec![0xBB, 0xCC]]);
        assert_ne!(batch_id(&left), batch_id(&right));
    }

    #[test]
    fn the_identifier_is_domain_separated_from_a_plain_hash() {
        let batch = batch_of(vec![vec![7]]);
        let mut plain = Hasher::new();
        plain.update(&1u64.to_le_bytes());
        plain.update(&1u64.to_le_bytes());
        plain.update(&[7]);
        assert_ne!(batch_id(&batch), *plain.finalize().as_bytes());
    }

    #[test]
    fn addresses_in_one_shard_produce_a_single_bit() {
        let address = [0u8; 32];
        let access = access_for_addresses(&[address]).expect("non-empty");
        assert_eq!(access.mask().count_ones(), 1);
        assert!(access.touches(shard_of(&address)));
    }

    #[test]
    fn addresses_in_different_shards_produce_distinct_bits() {
        let mut first = [0u8; 32];
        let mut second = [0u8; 32];
        first[0] = 0x00;
        second[0] = 0xFC;
        let access = access_for_addresses(&[first, second]).expect("non-empty");
        assert_eq!(access.mask().count_ones(), 2);
    }

    #[test]
    fn two_addresses_in_one_shard_collapse_to_one_bit() {
        // Sharing a shard is exactly what makes two transactions conflict, so
        // the mask must not double-count it.
        let mut first = [0u8; 32];
        let mut second = [0u8; 32];
        first[0] = 0x00;
        second[0] = 0x03; // same top six bits
        assert_eq!(shard_of(&first), shard_of(&second));
        let access = access_for_addresses(&[first, second]).expect("non-empty");
        assert_eq!(access.mask().count_ones(), 1);
    }

    #[test]
    fn an_empty_address_list_is_refused() {
        assert_eq!(access_for_addresses(&[]), Err(GraphError::EmptyAccessSet));
    }
}
