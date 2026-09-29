//! The grant journal: why a restart cannot reset the limits.
//!
//! The limiter and the daily budget live in memory. Before this journal
//! existed, restarting the faucet handed every IP and address a fresh window
//! and the day a fresh budget, so anyone who could make it restart got a new
//! quota (found on maya-testnet-1, 2026-09-29).
//!
//! Every grant is appended here and synced before it is reported, so a
//! grant the caller sees is a grant a restart remembers. On start the grants
//! of the last [`WINDOW`] are replayed into the limiter and the budget, and
//! the file is rewritten with only those, so it never grows past one day of
//! grants.

use std::fs::{File, OpenOptions};
use std::io::{BufRead as _, BufReader, Write as _};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::limit::WINDOW;

#[derive(Serialize, Deserialize)]
struct Entry {
    /// Unix seconds.
    at: u64,
    ip: IpAddr,
    address: String,
    amount: u64,
}

/// A grant read back from the journal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Replayed {
    /// When it was granted.
    pub at: SystemTime,
    /// The caller it was granted to.
    pub ip: IpAddr,
    /// The address funded.
    pub address: [u8; 32],
    /// Units granted.
    pub amount: u64,
}

/// The open journal.
#[derive(Debug)]
pub struct Ledger {
    file: File,
}

fn secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl Ledger {
    /// Opens (or creates) the journal at `path`, returns the grants of the
    /// last day, and compacts the file to just those.
    ///
    /// # Errors
    ///
    /// An unreadable or unwritable file, or a malformed line: a faucet that
    /// cannot read its own history must not start with an empty one.
    pub fn open(path: &Path, now: SystemTime) -> Result<(Self, Vec<Replayed>), String> {
        let mut live = Vec::new();
        if path.exists() {
            let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
            for (n, line) in BufReader::new(file).lines().enumerate() {
                let line = line.map_err(|e| format!("{}: {e}", path.display()))?;
                if line.trim().is_empty() {
                    continue;
                }
                let entry: Entry = serde_json::from_str(&line)
                    .map_err(|e| format!("{} line {}: {e}", path.display(), n + 1))?;
                let at = UNIX_EPOCH + Duration::from_secs(entry.at);
                // A clock that went backwards keeps the entry: refusing for a
                // day is recoverable, granting twice is not (limit.rs).
                let recent = now.duration_since(at).map_or(true, |age| age < WINDOW);
                if recent {
                    let address = hex::decode(&entry.address)
                        .ok()
                        .and_then(|b| <[u8; 32]>::try_from(b).ok())
                        .ok_or_else(|| format!("{} line {}: bad address", path.display(), n + 1))?;
                    live.push(Replayed {
                        at,
                        ip: entry.ip,
                        address,
                        amount: entry.amount,
                    });
                }
            }
        }
        let file = Self::rewrite(path, &live)?;
        Ok((Self { file }, live))
    }

    /// Writes `live` to a temporary file, syncs it and renames it over
    /// `path`, then reopens `path` for appending.
    fn rewrite(path: &Path, live: &[Replayed]) -> Result<File, String> {
        let tmp: PathBuf = path.with_extension("compacting");
        let mut out = File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        for r in live {
            writeln!(out, "{}", Self::line(r.at, r.ip, &r.address, r.amount)?)
                .map_err(|e| format!("{}: {e}", tmp.display()))?;
        }
        out.sync_all()
            .map_err(|e| format!("{}: {e}", tmp.display()))?;
        drop(out);
        std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))?;
        OpenOptions::new()
            .append(true)
            .open(path)
            .map_err(|e| format!("{}: {e}", path.display()))
    }

    fn line(at: SystemTime, ip: IpAddr, address: &[u8; 32], amount: u64) -> Result<String, String> {
        serde_json::to_string(&Entry {
            at: secs(at),
            ip,
            address: hex::encode(address),
            amount,
        })
        .map_err(|e| e.to_string())
    }

    /// Appends a grant and syncs it to disk.
    ///
    /// # Errors
    ///
    /// The write or the sync failed; the grant must then not be reported.
    pub fn record(
        &mut self,
        at: SystemTime,
        ip: IpAddr,
        address: &[u8; 32],
        amount: u64,
    ) -> Result<(), String> {
        let line = Self::line(at, ip, address, amount)?;
        writeln!(self.file, "{line}").map_err(|e| e.to_string())?;
        self.file.sync_data().map_err(|e| e.to_string())
    }
}
