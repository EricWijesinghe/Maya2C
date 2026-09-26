//! Slashing protection (Master Prompt 16 §2).
//!
//! The signer, not the node, decides whether a consensus message may be
//! signed. A validator is slashed for signing two different messages for the
//! same round, and every way that happens in practice is operational: a node
//! restored from an old backup, two nodes started with the same key, a crash
//! between signing and remembering. So the rule lives beside the key.
//!
//! # The rule
//!
//! For each `(kind, round)` at most one signing root is ever approved. A
//! request is approved when:
//!
//! - an identical record exists (the same root): re-signing it is harmless
//!   and makes a retried request idempotent; or
//! - no record exists for that `(kind, round)` **and** the round is above
//!   every round already signed for that kind.
//!
//! Anything else is refused: a different root for a signed round
//! ([`Refusal::Conflict`]) or a round at or below the high-water mark
//! ([`Refusal::BelowWatermark`]), which is what stops a node restored from a
//! backup from signing a past round it has forgotten it signed.
//!
//! # Durability
//!
//! [`SlashingDb::approve`] appends the record and calls `fsync` before it
//! returns `Ok`. The caller signs only after that. A crash between the write
//! and the signature leaves a record for a signature that was never released,
//! which can only refuse more, never less.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// What is being signed. Each kind has its own rounds and watermark.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A DAG vertex proposal.
    Vertex,
    /// A vote (certificate share) on another validator's vertex.
    Vote,
}

/// Why a request was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// A different message was already signed for this round.
    #[error("round {round} already signed with a different message")]
    Conflict {
        /// The round.
        round: u64,
    },
    /// The round is not above the highest round signed for this kind.
    #[error("round {round} is at or below the watermark {watermark}")]
    BelowWatermark {
        /// The round asked for.
        round: u64,
        /// The highest round signed.
        watermark: u64,
    },
}

/// Failures other than a refusal.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// The database file could not be read or written.
    #[error("slashing-protection storage: {0}")]
    Io(#[from] std::io::Error),
    /// A line in the database or an interchange file is malformed.
    #[error("slashing-protection record: {0}")]
    Format(String),
    /// The request was refused by the rule.
    #[error(transparent)]
    Refused(#[from] Refusal),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Record {
    kind: Kind,
    round: u64,
    #[serde(with = "hex32")]
    root: [u8; 32],
}

/// The persisted records for one validator key.
#[derive(Debug)]
pub struct SlashingDb {
    path: PathBuf,
    file: File,
    records: BTreeMap<(Kind, u64), Vec<[u8; 32]>>,
}

impl SlashingDb {
    /// Opens or creates the database at `path`, replaying every record.
    ///
    /// # Errors
    ///
    /// [`DbError::Io`] or [`DbError::Format`] for an unreadable file.
    pub fn open(path: &Path) -> Result<Self, DbError> {
        let mut records: BTreeMap<(Kind, u64), Vec<[u8; 32]>> = BTreeMap::new();
        if path.exists() {
            for line in BufReader::new(File::open(path)?).lines() {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                let r: Record =
                    serde_json::from_str(&line).map_err(|e| DbError::Format(e.to_string()))?;
                let roots = records.entry((r.kind, r.round)).or_default();
                if !roots.contains(&r.root) {
                    roots.push(r.root);
                }
            }
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            file,
            records,
        })
    }

    /// The highest round signed for `kind`, if any.
    #[must_use]
    pub fn watermark(&self, kind: Kind) -> Option<u64> {
        self.records
            .keys()
            .filter(|(k, _)| *k == kind)
            .map(|(_, r)| *r)
            .max()
    }

    /// Decides a request and, if approved, makes the approval durable.
    ///
    /// # Errors
    ///
    /// [`DbError::Refused`] when the rule refuses; [`DbError::Io`] when the
    /// record cannot be persisted, in which case nothing may be signed.
    pub fn approve(&mut self, kind: Kind, round: u64, root: [u8; 32]) -> Result<(), DbError> {
        if let Some(roots) = self.records.get(&(kind, round)) {
            // A single identical record: an idempotent retry. Two records for
            // one round can only come from an import and refuse everything.
            return if roots.as_slice() == [root] {
                Ok(())
            } else {
                Err(Refusal::Conflict { round }.into())
            };
        }
        if let Some(watermark) = self.watermark(kind)
            && round <= watermark
        {
            return Err(Refusal::BelowWatermark { round, watermark }.into());
        }
        self.persist(Record { kind, round, root })?;
        self.records.insert((kind, round), vec![root]);
        Ok(())
    }

    fn persist(&mut self, r: Record) -> Result<(), DbError> {
        let mut line = serde_json::to_string(&r).map_err(|e| DbError::Format(e.to_string()))?;
        line.push('\n');
        self.file.write_all(line.as_bytes())?;
        self.file.sync_all()?; // before any signature is released
        Ok(())
    }

    /// Exports every record in the interchange format.
    #[must_use]
    pub fn export(&self, pubkey: &[u8]) -> Interchange {
        let entries = |kind: Kind| {
            self.records
                .iter()
                .filter(|((k, _), _)| *k == kind)
                .flat_map(|((_, round), roots)| {
                    roots.iter().map(|root| Signed {
                        round: round.to_string(),
                        signing_root: to_hex(root),
                    })
                })
                .collect()
        };
        Interchange {
            metadata: Metadata {
                interchange_format_version: FORMAT.to_string(),
            },
            data: vec![ValidatorData {
                pubkey: to_hex(pubkey),
                signed_vertices: entries(Kind::Vertex),
                signed_votes: entries(Kind::Vote),
            }],
        }
    }

    /// Imports records for `pubkey` from another machine, keeping everything:
    /// the union of both histories refuses at least what either did.
    ///
    /// # Errors
    ///
    /// [`DbError::Format`] for a foreign format version or malformed entry.
    pub fn import(&mut self, file: &Interchange, pubkey: &[u8]) -> Result<usize, DbError> {
        if file.metadata.interchange_format_version != FORMAT {
            return Err(DbError::Format(format!(
                "unsupported interchange version {}",
                file.metadata.interchange_format_version
            )));
        }
        let mut added = 0;
        for v in file.data.iter().filter(|v| v.pubkey == to_hex(pubkey)) {
            for (kind, list) in [
                (Kind::Vertex, &v.signed_vertices),
                (Kind::Vote, &v.signed_votes),
            ] {
                for s in list {
                    let round = s
                        .round
                        .parse()
                        .map_err(|_| DbError::Format(format!("round {}", s.round)))?;
                    let root = from_hex(&s.signing_root)?;
                    let roots = self.records.entry((kind, round)).or_default();
                    if !roots.contains(&root) {
                        roots.push(root);
                        self.persist(Record { kind, round, root })?;
                        added += 1;
                    }
                }
            }
        }
        Ok(added)
    }

    /// Where the records live.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Interchange format version, modeled on EIP-3076 (`"5"`) with `Maya2C`'s
/// vertex/vote kinds in place of Ethereum's blocks/attestations.
pub const FORMAT: &str = "5-maya2c";

/// An interchange file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Interchange {
    /// Format metadata.
    pub metadata: Metadata,
    /// One entry per validator key.
    pub data: Vec<ValidatorData>,
}

/// Interchange metadata.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metadata {
    /// Always [`FORMAT`].
    pub interchange_format_version: String,
}

/// One validator's history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidatorData {
    /// Hex public key.
    pub pubkey: String,
    /// Signed vertex proposals.
    pub signed_vertices: Vec<Signed>,
    /// Signed votes.
    pub signed_votes: Vec<Signed>,
}

/// One signed message. Numbers are decimal strings, as in EIP-3076.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signed {
    /// Round, decimal.
    pub round: String,
    /// BLAKE3 of the domain-separated message, hex.
    pub signing_root: String,
}

fn to_hex(b: &[u8]) -> String {
    const D: &[u8; 16] = b"0123456789abcdef";
    b.iter()
        .flat_map(|x| {
            [
                char::from(D[usize::from(x >> 4)]),
                char::from(D[usize::from(x & 15)]),
            ]
        })
        .collect()
}

fn from_hex(s: &str) -> Result<[u8; 32], DbError> {
    if s.len() != 64 {
        return Err(DbError::Format(format!("signing root {s}")));
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[2 * i..2 * i + 2], 16)
            .map_err(|_| DbError::Format(format!("signing root {s}")))?;
    }
    Ok(out)
}

mod hex32 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::to_hex(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 32], D::Error> {
        let s = String::deserialize(d)?;
        super::from_hex(&s).map_err(serde::de::Error::custom)
    }
}
