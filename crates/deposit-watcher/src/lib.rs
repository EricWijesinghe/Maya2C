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

use maya_amount::Amount;
use serde::{Deserialize, Serialize};

/// A transfer as the watcher sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transfer {
    /// Transaction id, hex.
    pub txid: String,
    /// Recipient address, hex.
    pub to: String,
    /// Amount in base units: 10^24 per coin (`maya_amount::COIN_DECIMALS`).
    #[serde(with = "amount_text")]
    pub amount: Amount,
}

/// Amounts are 256-bit (ADR-046), so they travel as base-10 strings. A plain
/// JSON number is still read: journals written before this change held `u64`.
mod amount_text {
    use std::fmt;

    use maya_amount::Amount;
    use serde::de::{self, Visitor};
    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &Amount, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(value)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Amount, D::Error> {
        struct AmountVisitor;
        impl Visitor<'_> for AmountVisitor {
            type Value = Amount;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a base-10 amount below 2^256, as a string or a number")
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Amount, E> {
                Ok(Amount::from_u64(v))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Amount, E> {
                parse(v).ok_or_else(|| E::custom("amount is not a base-10 integer below 2^256"))
            }
        }
        d.deserialize_any(AmountVisitor)
    }

    /// Checked at every digit, so a value that would wrap is refused.
    fn parse(text: &str) -> Option<Amount> {
        if text.is_empty() {
            return None;
        }
        text.bytes().try_fold(Amount::ZERO, |acc, b| {
            let digit = b.checked_sub(b'0').filter(|d| *d < 10)?;
            acc.checked_mul(Amount::from_u64(10))?
                .checked_add(Amount::from_u64(u64::from(digit)))
        })
    }
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
    /// A balance would exceed 2^256 - 1. Refused before anything is journaled.
    #[error("balance of {0} overflows 256 bits")]
    Overflow(String),
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
    balances: BTreeMap<String, Amount>,
    seen: BTreeSet<String>,
}

/// Adds every credit to `balances`, refusing a sum that leaves 256 bits.
fn credit(balances: &mut BTreeMap<String, Amount>, credits: &[Transfer]) -> Result<(), WatchError> {
    for c in credits {
        let entry = balances.entry(c.to.clone()).or_insert(Amount::ZERO);
        *entry = entry
            .checked_add(c.amount)
            .ok_or_else(|| WatchError::Overflow(c.to.clone()))?;
    }
    Ok(())
}

impl Watcher {
    /// Opens the journal at `path`, replaying it and cutting off a torn tail.
    ///
    /// # Errors
    ///
    /// [`WatchError::Corrupt`] for a complete line that does not parse.
    pub fn open(path: &Path, watched: BTreeSet<String>) -> Result<Self, WatchError> {
        // `write`, not `append`: on Windows an append-only handle may not
        // truncate, so discarding a torn final line below failed with "Access
        // is denied". This watcher is the journal's only writer and seeks to
        // the end itself, which is all append mode was giving it.
        let mut journal = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
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
            credit(&mut balances, &line.credits)?;
            for c in &line.credits {
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

    /// Credited balance per watched address, in base units.
    #[must_use]
    pub fn balances(&self) -> &BTreeMap<String, Amount> {
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
            // Balances are computed before the line is written: an overflow
            // must leave the journal and the ledger both untouched.
            let mut next = self.balances.clone();
            credit(&mut next, &credits)?;
            let mut line = serde_json::to_string(&Line {
                height,
                credits: credits.clone(),
            })
            .map_err(|e| WatchError::Source(e.to_string()))?;
            line.push('\n');
            self.journal.write_all(line.as_bytes())?;
            self.journal.sync_all()?; // the commit point
            self.balances = next;
            self.seen.extend(credits.into_iter().map(|c| c.txid));
            self.cursor = height;
            done += 1;
        }
        Ok(done)
    }
}
