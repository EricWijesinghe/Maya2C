//! The swap journal: a JSON file, replaced atomically on every change.
//!
//! A watcher that forgets a swap on restart forgets to refund it. So a swap is
//! journalled *before* the lock that funds it is broadcast. A crash between the
//! two leaves a journalled lock that never appears, which the worker reports as
//! `LockMissing` — never a funded lock nobody is watching. A failed broadcast
//! is not rolled back out of the journal for the same reason: the transaction
//! may still be mined.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use maya_htlc_lattice::CommitmentId;

use crate::error::{Result, WatcherError};
use crate::swap::{Leg, Outcome, Phase, Swap};

/// Format version.
const JOURNAL_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Contents {
    version: u32,
    swaps: Vec<Swap>,
}

/// The swaps this watcher is responsible for.
#[derive(Debug)]
pub struct Journal {
    path: PathBuf,
    swaps: Vec<Swap>,
}

impl Journal {
    /// Opens a journal, or starts an empty one if the file does not exist.
    ///
    /// # Errors
    ///
    /// [`WatcherError::Journal`] for an unreadable file, malformed JSON, or an
    /// unknown version.
    pub fn open(path: &Path) -> Result<Self> {
        let swaps = match std::fs::read(path) {
            Ok(bytes) => {
                let contents: Contents =
                    serde_json::from_slice(&bytes).map_err(|e| failure(path, e))?;
                if contents.version != JOURNAL_VERSION {
                    return Err(failure(
                        path,
                        format!("unknown version {}", contents.version),
                    ));
                }
                contents.swaps
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(failure(path, error)),
        };
        Ok(Self {
            path: path.to_path_buf(),
            swaps,
        })
    }

    /// Every swap, active and finished.
    #[must_use]
    pub fn swaps(&self) -> &[Swap] {
        &self.swaps
    }

    /// The swap under a commitment, if journalled.
    #[must_use]
    pub fn get(&self, id: &CommitmentId) -> Option<&Swap> {
        self.swaps.iter().find(|swap| swap.commitment_id == *id)
    }

    /// Adds a swap and saves.
    ///
    /// # Errors
    ///
    /// [`WatcherError::DuplicateSwap`] for a commitment already journalled,
    /// finished or not, and a save failure.
    pub fn insert(&mut self, swap: Swap) -> Result<()> {
        if self.get(&swap.commitment_id).is_some() {
            return Err(WatcherError::DuplicateSwap(hex::encode(swap.commitment_id)));
        }
        self.swaps.push(swap);
        self.save()
    }

    /// Records the responder's lock on an initiator's swap, and saves.
    ///
    /// # Errors
    ///
    /// [`WatcherError::Refused`] if the swap is unknown or already paired, and
    /// a save failure.
    pub fn pair(&mut self, id: &CommitmentId, inbound: Leg) -> Result<()> {
        let swap = self
            .swaps
            .iter_mut()
            .find(|swap| swap.commitment_id == *id)
            .ok_or_else(|| WatcherError::Refused("no swap under that commitment".to_owned()))?;
        if swap.inbound.is_some() {
            return Err(WatcherError::Refused(
                "the swap already has an inbound lock".to_owned(),
            ));
        }
        swap.inbound = Some(inbound);
        self.save()
    }

    /// Marks a swap finished and saves.
    ///
    /// # Errors
    ///
    /// A save failure.
    pub fn finish(&mut self, id: &CommitmentId, outcome: Outcome) -> Result<()> {
        for swap in self
            .swaps
            .iter_mut()
            .filter(|swap| swap.commitment_id == *id)
        {
            swap.phase = Phase::Finished(outcome);
        }
        self.save()
    }

    /// Writes a sibling file and renames it over the journal, so a crash leaves
    /// either the old journal or the new one and never half of either.
    fn save(&self) -> Result<()> {
        let contents = Contents {
            version: JOURNAL_VERSION,
            swaps: self.swaps.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&contents).map_err(|e| failure(&self.path, e))?;
        let staging = self.path.with_extension("tmp");
        // Synced before the rename: a rename is atomic for the name, not for
        // data still in the page cache, and a power cut after it could leave
        // the journal's name pointing at an empty file.
        let mut file = std::fs::File::create(&staging).map_err(|e| failure(&staging, e))?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|e| failure(&staging, e))?;
        drop(file);
        std::fs::rename(&staging, &self.path).map_err(|e| failure(&self.path, e))
    }
}

fn failure(path: &Path, reason: impl ToString) -> WatcherError {
    WatcherError::Journal {
        path: path.display().to_string(),
        reason: reason.to_string(),
    }
}
