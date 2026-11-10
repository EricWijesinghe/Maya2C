//! The incident log: one JSON line per record, each committing to the one before.
//!
//! `DIR/incidents.jsonl` only grows. [`append`] is the sole writer, and it
//! checks the whole chain before adding a line. A status service writes
//! unsigned drafts from what the chain shows. Only a record signed by a key in
//! the operator list is published, so a copy of the file cannot forge a
//! published incident, and an edit anywhere shows as a broken link at the
//! first line it touched.
//!
//! shortcut: a truncated tail (newest lines removed) still verifies. `maya-incident verify` prints the head hash to pin off-machine; upgrade to a signed head checkpoint when a second operator exists.
//! shortcut: appends are not locked across processes. The status service and `maya-incident publish` can interleave only if both write in the same instant; `verify` catches any damage that causes.

use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::operator::{self, Operator};

/// The log's file name inside its directory.
pub const FILE: &str = "incidents.jsonl";
/// The head an empty log links from: 32 zero bytes, hex.
pub const GENESIS_HEAD: &str = "0000000000000000000000000000000000000000000000000000000000000000";
/// Domain tag on every published record's signature, so it cannot be replayed as another message.
const SIGN_DOMAIN: &str = "maya-status incident record v1";

/// Longest incident id, bytes.
pub const MAX_ID: usize = 64;
/// Longest title, bytes.
pub const MAX_TITLE: usize = 120;
/// Longest body, bytes.
pub const MAX_BODY: usize = 2_000;
/// Longest report link, bytes.
pub const MAX_REPORT: usize = 300;

/// Whether a record is an unsigned draft or an operator's publication.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Written by the status service from the chain. Unsigned; not trusted as an incident.
    Draft,
    /// Signed by an operator in the trusted list.
    Signed,
}

/// How loud an incident is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Worth a look.
    Warning,
    /// The chain was down.
    Critical,
}

/// What a record says. Its canonical bytes are this struct's JSON, fields in
/// declaration order, so the hash is reproducible from the line alone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Body {
    /// Position in the log, from 0.
    pub seq: u64,
    /// Hash of the record before, or [`GENESIS_HEAD`].
    pub prev: String,
    /// Draft or published.
    pub kind: Kind,
    /// The incident this record is about.
    pub id: String,
    /// When it was written, Unix seconds.
    pub recorded_at: u64,
    /// When the incident started, Unix seconds.
    pub started: u64,
    /// When it ended, Unix seconds; `None` while it is open.
    pub ended: Option<u64>,
    /// How loud.
    pub severity: Severity,
    /// One-line headline.
    pub title: String,
    /// What happened.
    pub body: String,
    /// Link to the full report; https only.
    pub report: Option<String>,
}

/// One line of the log: the body, its hash, and for a publication the
/// operator's public key and signature over the hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// The record's content.
    pub body: Body,
    /// Hex SHA-256 of the body's canonical JSON.
    pub hash: String,
    /// Hex public key of the signing operator, if published.
    pub signer: Option<String>,
    /// Hex ML-DSA-65 signature over the signed message, if published.
    pub sig: Option<String>,
}

/// What a caller asks to be recorded. [`append`] fills in the chain fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Draft or published.
    pub kind: Kind,
    /// The incident id.
    pub id: String,
    /// When it is written, Unix seconds.
    pub recorded_at: u64,
    /// Start, Unix seconds.
    pub started: u64,
    /// End, Unix seconds, if known.
    pub ended: Option<u64>,
    /// How loud.
    pub severity: Severity,
    /// Headline.
    pub title: String,
    /// Detail.
    pub body: String,
    /// Report link, https only.
    pub report: Option<String>,
}

/// Why the log could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The file could not be read or written.
    #[error("incident log unreadable: {0}")]
    Io(#[from] std::io::Error),
    /// A line does not verify. `line` is 1-based.
    #[error("incident log fails verification at line {line}: {reason}")]
    Tampered {
        /// 1-based line number.
        line: usize,
        /// What did not check out.
        reason: String,
    },
    /// The write was refused before it touched the file.
    #[error("refused: {0}")]
    Refused(String),
    /// A record could not be encoded.
    #[error("cannot encode record: {0}")]
    Encode(#[from] serde_json::Error),
}

impl Entry {
    /// Refuses what the log must never carry: a bad id, text past its bound
    /// or with control characters, a report link that is not https, or an
    /// end before the start.
    ///
    /// # Errors
    ///
    /// [`StoreError::Refused`] naming the first rule broken.
    pub fn validate(&self) -> Result<(), StoreError> {
        let refuse = |why: &str| Err(StoreError::Refused(why.to_owned()));
        let id_ok = !self.id.is_empty()
            && self.id.len() <= MAX_ID
            && self.id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
        if !id_ok {
            return refuse("id must be 1-64 ASCII letters, digits or '-'");
        }
        if self.title.is_empty() || self.title.len() > MAX_TITLE || self.title.chars().any(char::is_control) {
            return refuse("title must be 1-120 bytes, no control characters");
        }
        if self.body.len() > MAX_BODY || self.body.chars().any(|c| c.is_control() && c != '\n') {
            return refuse("body must be at most 2000 bytes, no control characters");
        }
        if self.ended.is_some_and(|e| e < self.started) {
            return refuse("ended is before started");
        }
        let report_ok = self
            .report
            .as_deref()
            .is_none_or(|url| url.starts_with("https://") && url.len() <= MAX_REPORT);
        if !report_ok {
            return refuse("report must be an https:// link of at most 300 bytes");
        }
        Ok(())
    }
}

/// Canonical hash of a record's body.
///
/// # Errors
///
/// The body cannot be encoded as JSON.
pub fn record_hash(body: &Body) -> Result<String, StoreError> {
    let bytes = serde_json::to_vec(body)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

/// The bytes an operator signs for a record with this hash.
fn signed_message(hash: &str) -> Vec<u8> {
    [SIGN_DOMAIN.as_bytes(), b"\n", hash.as_bytes()].concat()
}

/// Checks one record against the head it must link to and the trusted keys.
fn check(record: &Record, position: usize, head: &str, trusted: &[Vec<u8>]) -> Result<(), String> {
    let body = &record.body;
    if body.seq != position as u64 {
        return Err(format!("seq is {}, expected {position}", body.seq));
    }
    if body.prev != head {
        return Err("prev does not link to the line above (a line was removed, reordered or edited)".into());
    }
    let hash = record_hash(body).map_err(|e| e.to_string())?;
    if hash != record.hash {
        return Err("hash does not match the body (edited)".into());
    }
    match (body.kind, record.signer.as_deref(), record.sig.as_deref()) {
        (Kind::Draft, None, None) => Ok(()),
        (Kind::Signed, Some(signer), Some(sig)) => {
            let public = hex::decode(signer).map_err(|_| "signer is not hex".to_owned())?;
            if !trusted.contains(&public) {
                return Err("signer is not a trusted operator".into());
            }
            let sig = hex::decode(sig).map_err(|_| "signature is not hex".to_owned())?;
            if operator::verify(&public, &signed_message(&hash), &sig) {
                Ok(())
            } else {
                Err("signature does not verify".into())
            }
        }
        _ => Err("a draft carries a signature, or a published record has none".into()),
    }
}

fn tampered(line: usize, reason: String) -> StoreError {
    StoreError::Tampered { line, reason }
}

/// Every record in `dir`, in order, each checked against the chain so far and
/// the trusted operator keys. A missing log is an empty one.
///
/// # Errors
///
/// The file cannot be read, or any line fails verification.
pub fn load(dir: &Path, trusted: &[Vec<u8>]) -> Result<Vec<Record>, StoreError> {
    let text = match std::fs::read_to_string(dir.join(FILE)) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut records: Vec<Record> = Vec::new();
    let mut head = GENESIS_HEAD.to_owned();
    for (position, line) in text.lines().enumerate() {
        let line_no = position + 1;
        let record: Record = serde_json::from_str(line)
            .map_err(|e| tampered(line_no, format!("not a record: {e}")))?;
        check(&record, position, &head, trusted).map_err(|r| tampered(line_no, r))?;
        head.clone_from(&record.hash);
        records.push(record);
    }
    Ok(records)
}

/// Hash of the newest record, or [`GENESIS_HEAD`] for an empty log.
#[must_use]
pub fn head(records: &[Record]) -> String {
    records
        .last()
        .map_or_else(|| GENESIS_HEAD.to_owned(), |r| r.hash.clone())
}

/// Adds `entry` to the end of the log. A draft is unsigned. A published
/// record must be signed by `operator`, whose key must be trusted, or the
/// log would not verify afterwards.
///
/// # Errors
///
/// The entry is invalid, the log fails verification, the signer is not
/// trusted, or the file cannot be written.
pub fn append(
    dir: &Path,
    trusted: &[Vec<u8>],
    entry: &Entry,
    operator: Option<&Operator>,
) -> Result<Record, StoreError> {
    entry.validate()?;
    match (entry.kind, operator) {
        (Kind::Draft, Some(_)) => return Err(StoreError::Refused("a draft is never signed".into())),
        (Kind::Signed, None) => return Err(StoreError::Refused("a published record needs an operator key".into())),
        (Kind::Signed, Some(op)) if !trusted.iter().any(|k| k.as_slice() == op.public_key()) => {
            return Err(StoreError::Refused("operator key is not in the trusted list".into()));
        }
        _ => {}
    }
    let records = load(dir, trusted)?;
    let body = Body {
        seq: u64::try_from(records.len()).unwrap_or(u64::MAX),
        prev: head(&records),
        kind: entry.kind,
        id: entry.id.clone(),
        recorded_at: entry.recorded_at,
        started: entry.started,
        ended: entry.ended,
        severity: entry.severity,
        title: entry.title.clone(),
        body: entry.body.clone(),
        report: entry.report.clone(),
    };
    let hash = record_hash(&body)?;
    let (signer, sig) = match operator {
        Some(op) => {
            let sig = op
                .sign(&signed_message(&hash))
                .map_err(StoreError::Refused)?;
            (Some(hex::encode(op.public_key())), Some(hex::encode(sig)))
        }
        None => (None, None),
    };
    let record = Record { body, hash, signer, sig };
    let mut line = serde_json::to_string(&record)?;
    line.push('\n');
    std::fs::create_dir_all(dir)?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(FILE))?;
    file.write_all(line.as_bytes())?;
    file.sync_all()?;
    Ok(record)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn draft(id: &str) -> Entry {
        Entry {
            kind: Kind::Draft,
            id: id.into(),
            recorded_at: 100,
            started: 50,
            ended: None,
            severity: Severity::Critical,
            title: "Chain halted".into(),
            body: "No new block.".into(),
            report: None,
        }
    }

    fn fixture() -> (tempfile::TempDir, Operator, Vec<Vec<u8>>) {
        let dir = tempfile::tempdir().unwrap();
        let op = Operator::from_seed([9; 32]);
        let trusted = vec![op.public_key().to_vec()];
        (dir, op, trusted)
    }

    #[test]
    fn appends_grow_the_file_and_never_rewrite_it() {
        let (dir, op, trusted) = fixture();
        append(dir.path(), &trusted, &draft("halt-50"), None).unwrap();
        let before = std::fs::read(dir.path().join(FILE)).unwrap();
        let mut signed = draft("halt-50");
        signed.kind = Kind::Signed;
        signed.ended = Some(90);
        append(dir.path(), &trusted, &signed, Some(&op)).unwrap();
        let after = std::fs::read(dir.path().join(FILE)).unwrap();
        assert!(after.starts_with(&before), "earlier bytes must be untouched");
        let records = load(dir.path(), &trusted).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[1].body.prev, records[0].hash);
    }

    #[test]
    fn a_draft_is_never_signed_and_a_publication_always_is() {
        let (dir, op, trusted) = fixture();
        assert!(append(dir.path(), &trusted, &draft("a"), Some(&op)).is_err());
        let mut e = draft("a");
        e.kind = Kind::Signed;
        assert!(append(dir.path(), &trusted, &e, None).is_err());
    }

    #[test]
    fn an_untrusted_operator_cannot_publish() {
        let (dir, _op, trusted) = fixture();
        let stranger = Operator::from_seed([4; 32]);
        let mut e = draft("a");
        e.kind = Kind::Signed;
        assert!(append(dir.path(), &trusted, &e, Some(&stranger)).is_err());
    }

    #[test]
    fn an_edited_line_is_caught_at_that_line() {
        let (dir, _op, trusted) = fixture();
        append(dir.path(), &trusted, &draft("a"), None).unwrap();
        append(dir.path(), &trusted, &draft("b"), None).unwrap();
        let path = dir.path().join(FILE);
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replacen("Chain halted", "Nothing happened", 1)).unwrap();
        match load(dir.path(), &trusted) {
            Err(StoreError::Tampered { line: 1, .. }) => {}
            other => panic!("expected tamper at line 1, got {other:?}"),
        }
    }

    #[test]
    fn a_removed_or_reordered_line_breaks_the_link() {
        let (dir, _op, trusted) = fixture();
        for id in ["a", "b", "c"] {
            append(dir.path(), &trusted, &draft(id), None).unwrap();
        }
        let path = dir.path().join(FILE);
        let lines: Vec<String> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        std::fs::write(&path, format!("{}\n{}\n", lines[0], lines[2])).unwrap();
        assert!(matches!(
            load(dir.path(), &trusted),
            Err(StoreError::Tampered { line: 2, .. })
        ));
        std::fs::write(&path, format!("{}\n{}\n{}\n", lines[1], lines[0], lines[2])).unwrap();
        assert!(load(dir.path(), &trusted).is_err());
    }

    #[test]
    fn a_forged_publication_fails_its_signature() {
        let (dir, op, trusted) = fixture();
        let mut e = draft("a");
        e.kind = Kind::Signed;
        e.ended = Some(90);
        append(dir.path(), &trusted, &e, Some(&op)).unwrap();
        let path = dir.path().join(FILE);
        let mut record: Record =
            serde_json::from_str(std::fs::read_to_string(&path).unwrap().trim()).unwrap();
        record.body.title = "Nothing happened".into();
        // An attacker recomputes the hash after the edit, but has no key to re-sign.
        record.hash = record_hash(&record.body).unwrap();
        std::fs::write(&path, format!("{}\n", serde_json::to_string(&record).unwrap())).unwrap();
        let err = load(dir.path(), &trusted).unwrap_err().to_string();
        assert!(err.contains("signature does not verify"), "{err}");
    }

    #[test]
    fn a_draft_with_a_signature_or_a_signature_from_an_untrusted_key_is_refused() {
        let (dir, op, trusted) = fixture();
        let mut e = draft("a");
        e.kind = Kind::Signed;
        append(dir.path(), &trusted, &e, Some(&op)).unwrap();
        let path = dir.path().join(FILE);
        let mut record: Record =
            serde_json::from_str(std::fs::read_to_string(&path).unwrap().trim()).unwrap();
        record.signer = Some(hex::encode(Operator::from_seed([4; 32]).public_key()));
        std::fs::write(&path, format!("{}\n", serde_json::to_string(&record).unwrap())).unwrap();
        let err = load(dir.path(), &trusted).unwrap_err().to_string();
        assert!(err.contains("not a trusted operator"), "{err}");
    }

    #[test]
    fn validation_refuses_bad_ids_links_and_windows() {
        let mut e = draft("ok-1");
        assert!(e.validate().is_ok());
        e.id = "has space".into();
        assert!(e.validate().is_err());
        e.id = "ok".into();
        e.report = Some("http://insecure.example".into());
        assert!(e.validate().is_err());
        e.report = Some("https://status.maya2c.dev/r/1".into());
        e.ended = Some(10);
        assert!(e.validate().is_err(), "end before start");
    }

    #[test]
    fn a_missing_log_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(&dir.path().join("none"), &[]).unwrap().is_empty());
        assert_eq!(head(&[]), GENESIS_HEAD);
    }
}
