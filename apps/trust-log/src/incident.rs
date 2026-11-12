//! `signed-incidents`: an append-only, hash-chained log of outages and
//! attacks, one signed JSON file per record (`000001.json`, ...).
//!
//! Each record names its sequence number and the digest of the one before
//! it, so editing, removing or reordering any record breaks every later link
//! or signature. Dropping the *newest* records leaves a valid shorter chain;
//! that is caught only against the newest anchor (schema
//! `maya2c/incidents/v1/<seq>`, one per head), whose location the verifier
//! must learn from the chain or its own records — never from the log host —
//! or by passing the last sequence number it saw (`--min-seq`).
//!
//! Records point at heights, so anyone can re-execute the blocks involved
//! with `maya2c replay` (filled in when the draft names none).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::anchor::Anchor;
use crate::key::SignerKey;
use crate::signed::{self, Signed};
use crate::{Error, Result};

/// Digest domain of an incident record.
pub const DOMAIN: &str = "maya2c 2026-11-12 signed-incident v1";
/// Schema prefix of a head anchor; the head's seq follows, so each anchor
/// names one record (see [`crate::anchor`]).
pub const SCHEMA_PREFIX: &str = "maya2c/incidents/v1/";
/// `prev` of the first record.
pub const GENESIS_PREV: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// What the operator writes about an incident.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Draft {
    /// One-line headline.
    pub title: String,
    /// Heights involved, first to last.
    #[serde(default)]
    pub heights: Vec<u64>,
    /// Hex block ids at those heights, as the chain showed them.
    #[serde(default)]
    pub block_ids: Vec<String>,
    /// RPC captures: path and SHA-256 of each saved response.
    #[serde(default)]
    pub rpc_captures: Vec<String>,
    /// Timeline entries, oldest first.
    #[serde(default)]
    pub timeline: Vec<String>,
    /// The commit that fixed it, once there is one.
    #[serde(default)]
    pub fix_commit: Option<String>,
    /// Command lines that reproduce it.
    #[serde(default)]
    pub replay: Vec<String>,
}

/// One link of the log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Incident {
    /// 1 for the first record, then +1.
    pub seq: u64,
    /// Hex digest of record `seq - 1`, or [`GENESIS_PREV`].
    pub prev: String,
    /// The operator's account.
    pub record: Draft,
}

/// Replay lines for each height, when the draft has none.
#[must_use]
pub fn default_replay(heights: &[u64]) -> Vec<String> {
    heights
        .iter()
        .map(|h| format!("maya2c replay --from <RPC_URL> --genesis <GENESIS_FILE> {h}"))
        .collect()
}

/// Checks every link and signature, and returns the head digest.
///
/// # Errors
///
/// [`Error::Chain`] for an empty log, a gap, a reorder or a broken link;
/// [`Error::Signature`] for an edited or foreign record.
pub fn verify_chain(records: &[Signed<Incident>], pinned: &[u8]) -> Result<[u8; 32]> {
    if records.is_empty() {
        return Err(Error::Chain("the log is empty".into()));
    }
    let mut prev = GENESIS_PREV.to_owned();
    for (i, record) in (1u64..).zip(records) {
        if record.body.seq != i {
            return Err(Error::Chain(format!(
                "record {i} carries seq {}",
                record.body.seq
            )));
        }
        if record.body.prev != prev {
            return Err(Error::Chain(format!(
                "record {i} does not follow record {}",
                i - 1
            )));
        }
        record.open(DOMAIN, pinned)?;
        prev.clone_from(&record.digest);
    }
    crate::hex_array("digest", &prev)
}

/// The next record after `records`, signed.
///
/// # Errors
///
/// As [`verify_chain`] (a broken log is never extended) and [`signed::seal`].
pub fn next(
    records: &[Signed<Incident>],
    key: &SignerKey,
    draft: Draft,
) -> Result<Signed<Incident>> {
    let prev = if records.is_empty() {
        GENESIS_PREV.to_owned()
    } else {
        hex::encode(verify_chain(records, key.public_key())?)
    };
    let mut record = draft;
    if record.replay.is_empty() {
        record.replay = default_replay(&record.heights);
    }
    let seq = u64::try_from(records.len()).map_err(|e| Error::Malformed(e.to_string()))? + 1;
    signed::seal(key, DOMAIN, Incident { seq, prev, record })
}

/// Reads every record in `dir`, in file-name order.
///
/// # Errors
///
/// [`Error::Io`] or [`Error::Malformed`].
pub fn load(dir: &Path) -> Result<Vec<Signed<Incident>>> {
    let io = |e: std::io::Error| Error::Io(format!("{}: {e}", dir.display()));
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(io)?
        .map(|entry| entry.map(|e| e.path()).map_err(io))
        .collect::<Result<_>>()?;
    paths.retain(|p| p.extension().is_some_and(|x| x == "json"));
    paths.sort();
    paths
        .iter()
        .map(|p| {
            let text = std::fs::read_to_string(p).map_err(io)?;
            serde_json::from_str(&text)
                .map_err(|e| Error::Malformed(format!("{}: {e}", p.display())))
        })
        .collect()
}

/// Verifies the log in `dir`, appends `draft`, and returns the new file and
/// the anchor for the new head.
///
/// # Errors
///
/// As [`load`] and [`next`]; [`Error::Io`] if the file already exists.
pub fn append(dir: &Path, key: &SignerKey, draft: Draft) -> Result<(PathBuf, Anchor)> {
    let records = if dir.exists() { load(dir)? } else { Vec::new() };
    let record = next(&records, key, draft)?;
    std::fs::create_dir_all(dir).map_err(|e| Error::Io(format!("{}: {e}", dir.display())))?;
    let path = dir.join(format!("{:06}.json", record.body.seq));
    let text =
        serde_json::to_string_pretty(&record).map_err(|e| Error::Malformed(e.to_string()))?;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, text.as_bytes()))
        .map_err(|e| Error::Io(format!("{}: {e}", path.display())))?;
    let anchor = anchor_for(record.body.seq, crate::hex_array("digest", &record.digest)?)?;
    Ok((path, anchor))
}

/// The anchor for a head of sequence number `seq`.
///
/// # Errors
///
/// As [`Anchor::new`] (never for a prefix this short).
pub fn anchor_for(seq: u64, head: [u8; 32]) -> Result<Anchor> {
    Anchor::new(format!("{SCHEMA_PREFIX}{seq}"), head)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signed::tests::key;

    fn draft(title: &str) -> Draft {
        Draft {
            title: title.into(),
            heights: vec![100, 101],
            ..Draft::default()
        }
    }

    fn log(n: usize) -> Vec<Signed<Incident>> {
        let k = key(1);
        let mut records = Vec::new();
        for i in 0..n {
            records.push(next(&records, &k, draft(&format!("halt {i}"))).unwrap());
        }
        records
    }

    #[test]
    fn an_untouched_log_verifies_to_its_head() {
        let records = log(3);
        let head = verify_chain(&records, key(1).public_key()).unwrap();
        assert_eq!(hex::encode(head), records[2].digest);
        assert_eq!(records[0].body.prev, GENESIS_PREV);
        assert_eq!(records[2].body.seq, 3);
    }

    #[test]
    fn replay_lines_are_filled_from_heights() {
        let records = log(1);
        assert_eq!(records[0].body.record.replay, default_replay(&[100, 101]));
    }

    #[test]
    fn an_edited_record_breaks_the_log() {
        let mut records = log(3);
        records[1].body.record.title = "nothing happened".into();
        assert!(verify_chain(&records, key(1).public_key()).is_err());
    }

    #[test]
    fn a_re_signed_edit_breaks_the_next_link() {
        // The key holder rewrites record 2 and signs it again: record 3 still
        // points at the old digest.
        let k = key(1);
        let mut records = log(3);
        let mut body = records[1].body.clone();
        body.record.title = "nothing happened".into();
        records[1] = signed::seal(&k, DOMAIN, body).unwrap();
        assert!(matches!(
            verify_chain(&records, k.public_key()),
            Err(Error::Chain(_))
        ));
    }

    #[test]
    fn a_removed_or_reordered_record_breaks_the_log() {
        let records = log(3);
        let pinned = key(1).public_key().to_vec();
        let removed = [records[0].clone(), records[2].clone()];
        assert!(matches!(
            verify_chain(&removed, &pinned),
            Err(Error::Chain(_))
        ));
        let reordered = [records[1].clone(), records[0].clone(), records[2].clone()];
        assert!(matches!(
            verify_chain(&reordered, &pinned),
            Err(Error::Chain(_))
        ));
        assert!(matches!(verify_chain(&[], &pinned), Err(Error::Chain(_))));
    }

    #[test]
    fn a_broken_log_is_never_extended() {
        let mut records = log(2);
        records[0].body.record.title = "edited".into();
        assert!(next(&records, &key(1), draft("later")).is_err());
    }

    #[test]
    fn append_writes_files_that_load_and_verify() {
        let dir = tempfile::tempdir().unwrap();
        let k = key(1);
        append(dir.path(), &k, draft("a")).unwrap();
        let (path, anchor) = append(dir.path(), &k, draft("b")).unwrap();
        assert!(path.ends_with("000002.json"));
        let head = verify_chain(&load(dir.path()).unwrap(), k.public_key()).unwrap();
        assert_eq!(anchor.root, head);
        assert_eq!(anchor.schema, "maya2c/incidents/v1/2");
    }
}
