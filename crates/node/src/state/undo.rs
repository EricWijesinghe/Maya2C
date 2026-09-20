//! Undo journal for reverting a committed block.
//!
//! A reorg has to move state backwards, but a [`rocksdb::WriteBatch`] only
//! moves it forwards. So each journaled block records the *prior* value of
//! every account it touched, which is enough to reconstruct the pre-block state
//! exactly.
//!
//! An account that did not exist before the block is recorded with
//! `existed = false` and deleted on revert, rather than being restored as a
//! zero-balance record. The distinction matters: a spurious zero account would
//! change the Merkle state root and make the reverted state disagree with the
//! chain it is supposed to match.

use crate::core::codec::ByteReader;
use crate::error::Result;
use crate::state::account::{ACCOUNT_LEN, Account, Address};

/// Encoded size of one undo entry: address, presence flag, account record.
const ENTRY_SIZE: usize = 32 + 1 + ACCOUNT_LEN;

/// The prior value of a single account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UndoEntry {
    /// Account address.
    pub address: Address,
    /// Value before the block, or `None` if the account did not exist.
    pub previous: Option<Account>,
}

/// Everything needed to reverse one block's state changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UndoRecord {
    /// Prior values of every account the block touched.
    pub entries: Vec<UndoEntry>,
    /// The serialized shielded pool as it stood before the block, present only
    /// if the block touched it.
    ///
    /// The commitment tree is append-only and has no remove operation, so a
    /// reorg cannot un-append note by note. Storing the prior frontier and note
    /// count is what makes the rewind exact — and cheap, because the frontier
    /// is `TREE_DEPTH` nodes regardless of how many notes the pool holds.
    pub shielded: Option<Vec<u8>>,
    /// Nullifiers the block spent, deleted on revert so those notes become
    /// spendable again on the competing chain.
    pub nullifiers: Vec<[u8; 32]>,
    /// Prior values of every keyed record the block wrote: trading, oracle,
    /// governance, sealed-mempool, contract code and contract storage.
    ///
    /// Pool reserves are the reason this exists. Without it a reorg would leave
    /// a pool holding whatever the abandoned chain traded it to — which is not
    /// a detectable corruption, because the reserves would still be two
    /// plausible numbers, and the pool would go on quoting prices from them.
    ///
    /// Appended after the sections above, and decoded only if bytes remain, so
    /// a journal written before this field existed still reads.
    pub records: Vec<RecordUndo>,
}

/// The prior value of one keyed record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordUndo {
    /// Storage key: a generic record (`d:`, `o:`, `g:`, `m:`), contract code
    /// (`code:`) or a contract storage slot (`cstate:`).
    pub key: Vec<u8>,
    /// Value before the block, or `None` if the key did not exist.
    pub previous: Option<Vec<u8>>,
}

impl UndoRecord {
    /// Encodes the record for storage.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(8 + self.entries.len() * ENTRY_SIZE);
        buf.extend_from_slice(&(self.entries.len() as u64).to_le_bytes());

        for entry in &self.entries {
            buf.extend_from_slice(&entry.address);
            match &entry.previous {
                Some(account) => {
                    buf.push(1);
                    buf.extend_from_slice(&account.encode());
                }
                None => {
                    buf.push(0);
                    buf.extend_from_slice(&[0u8; ACCOUNT_LEN]);
                }
            }
        }

        match &self.shielded {
            Some(pool) => {
                buf.push(1);
                buf.extend_from_slice(&(pool.len() as u64).to_le_bytes());
                buf.extend_from_slice(pool);
            }
            None => buf.push(0),
        }

        buf.extend_from_slice(&(self.nullifiers.len() as u64).to_le_bytes());
        for nullifier in &self.nullifiers {
            buf.extend_from_slice(nullifier);
        }

        // Written only when there is something to write, so a block that
        // touched no trading state produces exactly the journal it did before
        // the trading subsystem existed.
        if !self.records.is_empty() {
            buf.extend_from_slice(&(self.records.len() as u64).to_le_bytes());
            for record in &self.records {
                buf.extend_from_slice(&(record.key.len() as u64).to_le_bytes());
                buf.extend_from_slice(&record.key);
                match &record.previous {
                    Some(value) => {
                        buf.push(1);
                        buf.extend_from_slice(&(value.len() as u64).to_le_bytes());
                        buf.extend_from_slice(value);
                    }
                    None => buf.push(0),
                }
            }
        }

        buf
    }

    /// Decodes a stored record.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::NodeError::Decode`] if the record is truncated
    /// or malformed.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut reader = ByteReader::new(bytes);
        let count = reader.read_collection_len(ENTRY_SIZE)?;

        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            let address = reader.read_array::<32>()?;
            let existed = reader.read_u8()? == 1;
            let account_bytes = reader.read_array::<ACCOUNT_LEN>()?;
            entries.push(UndoEntry {
                address,
                previous: if existed {
                    Some(Account::decode(&account_bytes)?)
                } else {
                    None
                },
            });
        }

        let shielded = match reader.read_u8()? {
            0 => None,
            1 => {
                let length = reader.read_collection_len(1)?;
                Some(reader.read_slice(length)?.to_vec())
            }
            other => {
                return Err(crate::error::NodeError::Decode(format!(
                    "invalid shielded presence flag {other}"
                )));
            }
        };

        let nullifier_count = reader.read_collection_len(32)?;
        let mut nullifiers = Vec::with_capacity(nullifier_count);
        for _ in 0..nullifier_count {
            nullifiers.push(reader.read_array::<32>()?);
        }

        // Absent on a journal from before trading existed, and on any journal
        // for a block that touched no trading state. Both read as an empty
        // list rather than as a truncated record.
        let mut records = Vec::new();
        if reader.remaining() > 0 {
            let count = reader.read_collection_len(9)?;
            records.reserve(count);
            for _ in 0..count {
                let key_length = reader.read_collection_len(1)?;
                let key = reader.read_slice(key_length)?.to_vec();
                let previous = match reader.read_u8()? {
                    0 => None,
                    1 => {
                        let length = reader.read_collection_len(1)?;
                        Some(reader.read_slice(length)?.to_vec())
                    }
                    other => {
                        return Err(crate::error::NodeError::Decode(format!(
                            "invalid record presence flag {other}"
                        )));
                    }
                };
                records.push(RecordUndo { key, previous });
            }
        }

        reader.finish()?;
        Ok(Self {
            entries,
            shielded,
            nullifiers,
            records,
        })
    }
}
