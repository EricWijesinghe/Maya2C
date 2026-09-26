//! Exchange deposit reference service (Master Prompt 17 §1).
//!
//! Watches finalized blocks and credits every deposit to a watched address
//! **exactly once**, across crashes at any instruction.
//!
//! # How exactly-once holds
//!
//! The service's state is an append-only journal, one line per processed
//! block: the height and every credit that block produced. A block is
//! committed by writing its line and calling `fsync`. On start the journal is
//! replayed; a final line that is incomplete (a crash mid-write) is cut off
//! and that block is processed again. So:
//!
//! - a block whose line reached the disk is never processed twice (the cursor
//!   is past it), and
//! - a block whose line did not is processed again in full (nothing of it was
//!   recorded).
//!
//! Credits and cursor live in one line, so there is no window where one moved
//! and the other did not. Transaction ids are also remembered, so a source
//! that re-delivers a transaction in a later block cannot double-credit it.
//!
//! Only **finalized** heights are read: under the current proof-of-work chain
//! "finalized" is a confirmation depth the operator chooses
//! (`docs/integrations/EXCHANGES.md`); under DAG-BFT it will be the commit.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

/// A transfer as the watcher sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transfer {
    /// Transaction id, hex.
    pub txid: String,
    /// Recipient address, hex.
    pub to: String,
    /// Amount in base units.
    pub amount: u64,
}

/// Where finalized blocks come from: a node's RPC in production, a
/// generator in tests.
pub trait BlockSource {
    /// Highest height the operator treats as irreversible.
    fn finalized_height(&self) -> u64;
    /// The transfers in block `height`.
    ///
    /// # Errors
    ///
    /// A message if the block cannot be fetched now; the watcher retries later.
    fn transfers(&self, height: u64) -> Result<Vec<Transfer>, String>;
}

/// Failures.
#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    /// Journal I/O.
    #[error("deposit journal: {0}")]
    Io(#[from] std::io::Error),
    /// A complete journal line that does not parse: refuse to guess.
    #[error("deposit journal corrupt at line {0}")]
    Corrupt(usize),
    /// The source failed.
    #[error("block source: {0}")]
    Source(String),
}

#[derive(Serialize, Deserialize)]
struct Line {
    height: u64,
    credits: Vec<Transfer>,
}

/// The watcher and its durable state.
pub struct Watcher {
    journal: File,
    watched: BTreeSet<String>,
    cursor: u64,
    balances: BTreeMap<String, u128>,
    seen: BTreeSet<String>,
}

impl Watcher {
    /// Opens the journal at `path`, replaying it and cutting off a torn tail.
    ///
    /// # Errors
    ///
    /// [`WatchError::Corrupt`] for a complete line that does not parse.
    pub fn open(path: &Path, watched: BTreeSet<String>) -> Result<Self, WatchError> {
        let mut journal = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(path)?;
        let (mut cursor, mut balances, mut seen) = (0, BTreeMap::new(), BTreeSet::new());
        let mut good_len = 0u64;
        let mut reader = BufReader::new(File::open(path)?);
        let mut buf = String::new();
        let mut n = 0;
        loop {
            buf.clear();
            let read = reader.read_line(&mut buf)?;
            if read == 0 || !buf.ends_with('\n') {
                break; // end, or a torn final line: discard it
            }
            n += 1;
            let line: Line =
                serde_json::from_str(buf.trim_end()).map_err(|_| WatchError::Corrupt(n))?;
            for c in &line.credits {
                *balances.entry(c.to.clone()).or_insert(0u128) += u128::from(c.amount);
                seen.insert(c.txid.clone());
            }
            cursor = line.height;
            good_len += read as u64;
        }
        if journal.metadata()?.len() != good_len {
            journal.set_len(good_len)?;
            journal.sync_all()?;
        }
        journal.seek(SeekFrom::End(0))?;
        Ok(Self {
            journal,
            watched,
            cursor,
            balances,
            seen,
        })
    }

    /// The last height fully processed.
    #[must_use]
    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    /// Credited balance per watched address.
    #[must_use]
    pub fn balances(&self) -> &BTreeMap<String, u128> {
        &self.balances
    }

    /// Number of deposits credited.
    #[must_use]
    pub fn credited(&self) -> usize {
        self.seen.len()
    }

    /// Processes every finalized block after the cursor.
    ///
    /// # Errors
    ///
    /// [`WatchError`] on I/O or source failure; state stays consistent.
    pub fn poll(&mut self, source: &impl BlockSource) -> Result<u64, WatchError> {
        let top = source.finalized_height();
        let mut done = 0;
        while self.cursor < top {
            let height = self.cursor + 1;
            let credits: Vec<Transfer> = source
                .transfers(height)
                .map_err(WatchError::Source)?
                .into_iter()
                .filter(|t| self.watched.contains(&t.to) && !self.seen.contains(&t.txid))
                .collect();
            let mut line = serde_json::to_string(&Line {
                height,
                credits: credits.clone(),
            })
            .map_err(|e| WatchError::Source(e.to_string()))?;
            line.push('\n');
            self.journal.write_all(line.as_bytes())?;
            self.journal.sync_all()?; // the commit point
            for c in credits {
                *self.balances.entry(c.to.clone()).or_insert(0) += u128::from(c.amount);
                self.seen.insert(c.txid);
            }
            self.cursor = height;
            done += 1;
        }
        Ok(done)
    }
}
