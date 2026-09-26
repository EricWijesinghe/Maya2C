//! A deterministic synthetic state of any size, generated on demand.
//!
//! 100 million accounts are 4.8 GB of records; nothing here holds them. A
//! chunk is regenerated from its index whenever it is needed, byte-identical
//! every time, which is what lets one machine play the network's servers and
//! the joining node for a state that does not fit in its memory.

/// Bytes per account record: 32-byte key, balance, nonce.
pub const ACCOUNT_BYTES: usize = 48;

/// Accounts `0 .. accounts`, `per_chunk` to a chunk.
#[derive(Clone, Copy, Debug)]
pub struct SyntheticState {
    /// Total accounts.
    pub accounts: u64,
    /// Accounts per chunk.
    pub per_chunk: u64,
}

impl SyntheticState {
    /// Number of chunks.
    pub fn chunks(&self) -> u32 {
        u32::try_from(self.accounts.div_ceil(self.per_chunk)).unwrap_or(u32::MAX)
    }

    /// Chunk `index`'s bytes.
    pub fn chunk(&self, index: u32) -> Vec<u8> {
        let first = u64::from(index) * self.per_chunk;
        let last = (first + self.per_chunk).min(self.accounts);
        let mut out =
            Vec::with_capacity(usize::try_from(last - first).unwrap_or(0) * ACCOUNT_BYTES);
        for i in first..last {
            out.extend_from_slice(blake3::hash(&i.to_le_bytes()).as_bytes());
            out.extend_from_slice(&(i.wrapping_mul(2_654_435_761) % 1_000_000_000).to_le_bytes());
            out.extend_from_slice(&(i % 97).to_le_bytes());
        }
        out
    }

    /// Sum of balances in a chunk: what an importer folds, to show it read
    /// every record.
    pub fn balance_sum(chunk: &[u8]) -> u128 {
        chunk
            .as_chunks::<ACCOUNT_BYTES>()
            .0
            .iter()
            .map(|r| {
                let mut b = [0u8; 8];
                b.copy_from_slice(&r[32..40]);
                u128::from(u64::from_le_bytes(b))
            })
            .sum()
    }
}
