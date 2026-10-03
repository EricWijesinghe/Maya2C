//! What a validator must remember across a restart to stay honest.
//!
//! Two logs per epoch, under `<data-dir>/bft/epoch-<n>/`:
//!
//! - `safety.log` — every proposal this validator signed and every vote it
//!   cast, **fsync'd before the message leaves the node**. A validator that
//!   restarts without them would propose round 1 again and vote again for
//!   slots it already voted in, and a second signature for one slot is an
//!   equivocation: slashable, and the one thing certification exists to
//!   prevent. This is the same rule as the remote signer's slashing
//!   protection (Master Prompt 16), for the same reason.
//! - `certs.log` — certificates seen, flushed but not fsync'd. Losing its tail
//!   costs a re-fetch from peers, never safety.
//!
//! Records are `len:u64 frame`, where the frame is a [`super::wire`] encoding.
//! A torn final record — the process died mid-write — is dropped on replay;
//! for `safety.log` that record was never sent, because sending waits for the
//! fsync.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use maya_dag_bft::Certificate;

use super::wire::{Envelope, decode_certificate, encode_certificate};
use crate::error::{NodeError, Result};

const SAFETY_LOG: &str = "safety.log";
const CERTS_LOG: &str = "certs.log";

/// The on-disk logs for one epoch.
#[derive(Debug)]
pub struct SafetyStore {
    dir: PathBuf,
    safety: File,
    certs: BufWriter<File>,
}

/// What a restart reads back.
#[derive(Debug, Default)]
pub struct Recovered {
    /// Own proposals and votes, oldest first, as the frames that carried them.
    pub safety: Vec<Envelope>,
    /// Certificates seen, oldest first.
    pub certificates: Vec<Certificate>,
}

fn io_err(what: &str, path: &Path, e: &std::io::Error) -> NodeError {
    NodeError::Storage(format!("{what} {}: {e}", path.display()))
}

fn open_append(path: &Path) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| io_err("open", path, &e))
}

/// Reads `len:u64 frame` records, stopping at a torn tail.
fn read_records(path: &Path) -> Result<Vec<Vec<u8>>> {
    let mut bytes = Vec::new();
    match File::open(path) {
        Ok(mut f) => f
            .read_to_end(&mut bytes)
            .map_err(|e| io_err("read", path, &e))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io_err("open", path, &e)),
    };
    let mut records = Vec::new();
    let mut rest = bytes.as_slice();
    while let Some((len, tail)) = rest.split_first_chunk::<8>() {
        let Ok(len) = usize::try_from(u64::from_le_bytes(*len)) else {
            break;
        };
        let Some((record, after)) = tail.split_at_checked(len) else {
            break; // torn tail
        };
        records.push(record.to_vec());
        rest = after;
    }
    Ok(records)
}

fn frame(record: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + record.len());
    out.extend_from_slice(&(record.len() as u64).to_le_bytes());
    out.extend_from_slice(record);
    out
}

/// Past epochs whose logs are kept beside the current one. One: a peer
/// still finishing the previous epoch may fetch its certificates. Older
/// logs serve nothing — an engine never signs for an epoch it has left, so
/// their votes cannot be repeated — and left alone they grow without bound
/// (maya-testnet-1: 3.1 GB of logs against 162 MB of state in 4.5 days).
pub const RETAINED_PAST_EPOCHS: u64 = 1;

/// Removes `root/epoch-<n>` for every `n < first_kept`, returning the epochs
/// removed. Anything in `root` that is not an epoch directory is left alone.
///
/// # Errors
///
/// [`NodeError::Storage`] naming the first directory that could not be read
/// or removed; directories removed before it stay removed.
pub fn prune_epochs_before(root: &Path, first_kept: u64) -> Result<Vec<u64>> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io_err("read", root, &e)),
    };
    let mut removed = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| io_err("read", root, &e))?;
        let name = entry.file_name();
        let Some(epoch) = name
            .to_str()
            .and_then(|n| n.strip_prefix("epoch-"))
            .and_then(|n| n.parse::<u64>().ok())
        else {
            continue;
        };
        if epoch < first_kept && entry.path().is_dir() {
            std::fs::remove_dir_all(entry.path())
                .map_err(|e| io_err("remove", &entry.path(), &e))?;
            removed.push(epoch);
        }
    }
    removed.sort_unstable();
    Ok(removed)
}

impl SafetyStore {
    /// Opens (creating if needed) `root/epoch-<epoch>` and reads back what an
    /// earlier run left there.
    ///
    /// # Errors
    ///
    /// [`NodeError::Storage`] if the directory or logs cannot be created or
    /// read. A record that does not decode is an error too: `safety.log` is
    /// only ever written by this code, so a bad record means the file is not
    /// what it claims to be, and guessing past it could forget a vote.
    pub fn open(root: &Path, epoch: u64) -> Result<(Self, Recovered)> {
        let dir = root.join(format!("epoch-{epoch}"));
        std::fs::create_dir_all(&dir).map_err(|e| io_err("create", &dir, &e))?;
        let safety_path = dir.join(SAFETY_LOG);
        let certs_path = dir.join(CERTS_LOG);
        let safety = read_records(&safety_path)?
            .iter()
            .map(|r| Envelope::decode(r))
            .collect::<Result<Vec<_>>>()?;
        // Certificates are re-verified on replay, and a torn or foreign one
        // costs only a re-fetch, so an undecodable record is skipped.
        let certificates = read_records(&certs_path)?
            .iter()
            .filter_map(|r| decode_certificate(r).ok())
            .collect();
        let store = Self {
            safety: open_append(&safety_path)?,
            certs: BufWriter::new(open_append(&certs_path)?),
            dir,
        };
        Ok((
            store,
            Recovered {
                safety,
                certificates,
            },
        ))
    }

    /// Appends one of this validator's own proposals or votes and waits for
    /// the disk. Call before the frame is sent.
    ///
    /// # Errors
    ///
    /// [`NodeError::Storage`] if the write or the fsync fails. The caller must
    /// then not send the frame.
    pub fn record_own(&mut self, envelope: &Envelope) -> Result<()> {
        let path = self.dir.join(SAFETY_LOG);
        self.safety
            .write_all(&frame(&envelope.encode()))
            .and_then(|()| self.safety.sync_data())
            .map_err(|e| io_err("append", &path, &e))
    }

    /// Appends a certificate. Buffered; see the module note.
    ///
    /// # Errors
    ///
    /// [`NodeError::Storage`] if the write fails.
    pub fn record_certificate(&mut self, c: &Certificate) -> Result<()> {
        let path = self.dir.join(CERTS_LOG);
        self.certs
            .write_all(&frame(&encode_certificate(c)))
            .and_then(|()| self.certs.flush())
            .map_err(|e| io_err("append", &path, &e))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use maya_dag_bft::{Message, Vertex};

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("maya-bft-store-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn vote(round: u64) -> Envelope {
        Envelope {
            epoch: 0,
            from: 1,
            to: 0,
            message: Message::Vote {
                digest: [round.to_le_bytes()[0]; 32],
                round,
                voter: 1,
                signature: vec![1, 2, 3],
            },
        }
    }

    #[test]
    fn only_epochs_before_the_kept_one_are_pruned() {
        let root = dir("prune");
        for epoch in 0..5 {
            let (mut store, _) = SafetyStore::open(&root, epoch).unwrap();
            store.record_own(&vote(1)).unwrap();
        }
        std::fs::write(root.join("notes.txt"), b"not an epoch").unwrap();
        std::fs::create_dir_all(root.join("epoch-x")).unwrap();

        assert_eq!(prune_epochs_before(&root, 3).unwrap(), vec![0, 1, 2]);
        for epoch in 0..3 {
            assert!(!root.join(format!("epoch-{epoch}")).exists());
        }
        // The kept epochs still hold their votes; other entries are untouched.
        let (_, recovered) = SafetyStore::open(&root, 3).unwrap();
        assert_eq!(recovered.safety.len(), 1);
        assert!(root.join("epoch-4").exists());
        assert!(root.join("notes.txt").exists() && root.join("epoch-x").exists());
        assert!(prune_epochs_before(&root, 3).unwrap().is_empty());
        assert!(
            prune_epochs_before(&root.join("absent"), 9)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn own_votes_and_certificates_survive_a_reopen() {
        let root = dir("reopen");
        let cert = Certificate {
            vertex: Vertex::genesis_in(0, 2),
            votes: vec![0],
            signatures: vec![vec![9]],
        };
        {
            let (mut store, recovered) = SafetyStore::open(&root, 0).unwrap();
            assert!(recovered.safety.is_empty() && recovered.certificates.is_empty());
            store.record_own(&vote(1)).unwrap();
            store.record_own(&vote(2)).unwrap();
            store.record_certificate(&cert).unwrap();
        }
        let (_, recovered) = SafetyStore::open(&root, 0).unwrap();
        assert_eq!(recovered.safety, vec![vote(1), vote(2)]);
        assert_eq!(recovered.certificates, vec![cert]);
        // Another epoch is another directory.
        let (_, other) = SafetyStore::open(&root, 1).unwrap();
        assert!(other.safety.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_torn_tail_is_dropped_and_what_precedes_it_kept() {
        let root = dir("torn");
        {
            let (mut store, _) = SafetyStore::open(&root, 0).unwrap();
            store.record_own(&vote(1)).unwrap();
        }
        let path = root.join("epoch-0").join(SAFETY_LOG);
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        // A length promising 100 bytes, then only 3: the process died here.
        f.write_all(&100u64.to_le_bytes()).unwrap();
        f.write_all(&[1, 2, 3]).unwrap();
        drop(f);
        let (_, recovered) = SafetyStore::open(&root, 0).unwrap();
        assert_eq!(recovered.safety, vec![vote(1)]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
