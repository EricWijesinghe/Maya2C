//! Slashing protection (Master Prompt 16 §2).
//!
//! The signer, not the node, decides whether a consensus message may be
//! signed. Every way a validator ends up slashable in practice is
//! operational: a node restored from an old backup, two nodes started with
//! the same key, a crash between signing and remembering. So the rule lives
//! beside the key.
//!
//! # The rule (ADR-032)
//!
//! The slashable act in DAG-BFT is signing two different digests for one
//! `(round, author)` slot: equivocating as an author, or voting for two
//! conflicting vertices of one author in one round. A proposal and its
//! author's own vote are the same signature, so they share the slot. A
//! request is approved when:
//!
//! - its slot holds exactly this digest already: re-signing it is harmless
//!   and makes a retried request idempotent; or
//! - its slot is empty, **and**, for a proposal only, the round is above
//!   every round this key has proposed in.
//!
//! Anything else is refused: a different digest for a filled slot
//! ([`Refusal::Conflict`]), or a proposal at or below the proposal watermark
//! ([`Refusal::BelowWatermark`]). Votes have no watermark: a validator votes
//! for every author's vertex in a round, and a slow author's vertex can be
//! voted on after later rounds. A one-per-round rule for votes, which the
//! first version had, would have refused honest votes and stalled
//! consensus. A node restored from a backup is still stopped, because the
//! slot records live here, not on the node.
//!
//! # Bounded history
//!
//! Records below a floor of [`RETAIN_ROUNDS`] under the highest round are
//! dropped, and every request below the floor is refused
//! ([`Refusal::BelowFloor`]). Forgetting them is safe for that reason: the
//! engine never signs further back than `GC_DEPTH` (50 rounds) behind its
//! last commit, and the floor is twenty times that. Requests more than
//! [`MAX_AHEAD`] rounds past the highest round, or for an author id at or
//! above [`MAX_AUTHORS`], are refused too, so an authenticated but
//! misbehaving node cannot grow the history without bound. The file is
//! compacted when it holds more than twice the live records.
//!
//! # Durability
//!
//! [`SlashingDb::approve`] appends the record and calls `fsync` before it
//! returns `Ok`. The caller signs only after that. A crash between the write
//! and the signature leaves a record for a signature that was never released,
//! which can only refuse more, never less.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// What is being signed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A DAG vertex proposal.
    Vertex,
    /// A vote (certificate share) on another validator's vertex.
    Vote,
}

/// Rounds kept below the highest round signed.
pub const RETAIN_ROUNDS: u64 = 1_000;
/// Furthest a request may reach past the highest round signed.
pub const MAX_AHEAD: u64 = 1_000;
/// Author ids at or above this are refused; far beyond any committee.
pub const MAX_AUTHORS: u16 = 1_024;
/// Lines the file may hold beyond twice the live records before compaction.
const COMPACT_SLACK: usize = 1_024;

/// Why a request was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// A different digest was already signed for this `(round, author)`.
    #[error("round {round}, author {author} already signed with a different digest")]
    Conflict {
        /// The round.
        round: u64,
        /// The vertex's author.
        author: u16,
    },
    /// A proposal not above the highest round this key has proposed in.
    #[error("proposal round {round} is at or below the watermark {watermark}")]
    BelowWatermark {
        /// The round asked for.
        round: u64,
        /// The highest round signed.
        watermark: u64,
    },
    /// Below the floor of retained history (see the module docs).
    #[error("round {round} is below the retained floor {floor}")]
    BelowFloor {
        /// The round asked for.
        round: u64,
        /// The floor.
        floor: u64,
    },
    /// Too far past the highest round, or an author id out of range.
    #[error("round {round}, author {author} is out of range")]
    OutOfRange {
        /// The round asked for.
        round: u64,
        /// The author asked for.
        author: u16,
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
    author: u16,
    #[serde(with = "hex32")]
    root: [u8; 32],
}

/// The persisted records for one validator key.
#[derive(Debug)]
pub struct SlashingDb {
    path: PathBuf,
    file: File,
    /// Digests approved per `(round, author)` slot. More than one can only
    /// come from an import, and then the slot refuses everything.
    slots: BTreeMap<(u64, u16), Vec<[u8; 32]>>,
    /// Every live record, round first so the history below the floor can be
    /// split off.
    records: BTreeSet<(u64, u16, Kind, [u8; 32])>,
    /// Lines in the file, for deciding when to compact.
    lines: usize,
}

impl SlashingDb {
    /// Opens or creates the database at `path`, replaying every record.
    ///
    /// # Errors
    ///
    /// [`DbError::Io`] or [`DbError::Format`] for an unreadable file.
    pub fn open(path: &Path) -> Result<Self, DbError> {
        let file_exists = path.exists();
        let mut db = Self {
            path: path.to_path_buf(),
            file: OpenOptions::new().create(true).append(true).open(path)?,
            slots: BTreeMap::new(),
            records: BTreeSet::new(),
            lines: 0,
        };
        if file_exists {
            for line in BufReader::new(File::open(path)?).lines() {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                let r: Record =
                    serde_json::from_str(&line).map_err(|e| DbError::Format(e.to_string()))?;
                db.remember(r);
                db.lines += 1;
            }
        }
        db.prune();
        db.compact_if_needed()?;
        Ok(db)
    }

    /// Adds a record to memory; `true` if it was new.
    fn remember(&mut self, r: Record) -> bool {
        let roots = self.slots.entry((r.round, r.author)).or_default();
        if !roots.contains(&r.root) {
            roots.push(r.root);
        }
        self.records.insert((r.round, r.author, r.kind, r.root))
    }

    /// The highest round signed, of either kind.
    fn top(&self) -> Option<u64> {
        self.records.last().map(|(round, ..)| *round)
    }

    /// The lowest round still decided here.
    #[must_use]
    pub fn floor(&self) -> u64 {
        self.top()
            .map_or(0, |top| top.saturating_sub(RETAIN_ROUNDS))
    }

    /// Drops the history below the floor from memory.
    fn prune(&mut self) {
        let floor = self.floor();
        self.slots = self.slots.split_off(&(floor, 0));
        self.records = self.records.split_off(&(floor, 0, Kind::Vertex, [0; 32]));
    }

    /// Rewrites the file with the live records once it holds more than
    /// twice as many lines: written to a temporary file, synced, then renamed
    /// over the old one, so a crash leaves one complete file or the other.
    fn compact_if_needed(&mut self) -> Result<(), DbError> {
        if self.lines <= 2 * self.records.len() + COMPACT_SLACK {
            return Ok(());
        }
        let tmp = self.path.with_extension("compacting");
        let mut out = File::create(&tmp)?;
        for (round, author, kind, root) in &self.records {
            let r = Record {
                kind: *kind,
                round: *round,
                author: *author,
                root: *root,
            };
            let mut line = serde_json::to_string(&r).map_err(|e| DbError::Format(e.to_string()))?;
            line.push('\n');
            out.write_all(line.as_bytes())?;
        }
        out.sync_all()?;
        drop(out);
        std::fs::rename(&tmp, &self.path)?;
        self.file = OpenOptions::new().append(true).open(&self.path)?;
        self.lines = self.records.len();
        Ok(())
    }

    /// The highest round this key has proposed in, for [`Kind::Vertex`].
    /// Votes have no watermark (see the module docs), so for [`Kind::Vote`]
    /// this is always `None`.
    #[must_use]
    pub fn watermark(&self, kind: Kind) -> Option<u64> {
        match kind {
            Kind::Vote => None,
            Kind::Vertex => self
                .records
                .iter()
                .filter(|(_, _, k, _)| *k == Kind::Vertex)
                .map(|(round, ..)| *round)
                .max(),
        }
    }

    /// Decides a request for the `(round, author)` slot and, if approved,
    /// makes the approval durable before returning.
    ///
    /// # Errors
    ///
    /// [`DbError::Refused`] when the rule refuses; [`DbError::Io`] when the
    /// record cannot be persisted, in which case nothing may be signed.
    pub fn approve(
        &mut self,
        kind: Kind,
        round: u64,
        author: u16,
        root: [u8; 32],
    ) -> Result<(), DbError> {
        let floor = self.floor();
        if round < floor {
            return Err(Refusal::BelowFloor { round, floor }.into());
        }
        if author >= MAX_AUTHORS
            || self
                .top()
                .is_some_and(|top| round > top.saturating_add(MAX_AHEAD))
        {
            return Err(Refusal::OutOfRange { round, author }.into());
        }
        if let Some(roots) = self.slots.get(&(round, author)) {
            // A single identical digest: an idempotent retry, or a proposal's
            // own vote. Two digests can only come from an import.
            return if roots.as_slice() == [root] {
                Ok(())
            } else {
                Err(Refusal::Conflict { round, author }.into())
            };
        }
        if let Some(watermark) = self.watermark(kind)
            && round <= watermark
        {
            return Err(Refusal::BelowWatermark { round, watermark }.into());
        }
        let record = Record {
            kind,
            round,
            author,
            root,
        };
        self.persist(record)?;
        self.remember(record);
        self.prune();
        // Compaction failing leaves the appended file intact and correct; it
        // only costs disk, so it must not stop this signature.
        if let Err(e) = self.compact_if_needed() {
            eprintln!("slashing protection: compaction failed, will retry: {e}");
        }
        Ok(())
    }

    fn persist(&mut self, r: Record) -> Result<(), DbError> {
        let mut line = serde_json::to_string(&r).map_err(|e| DbError::Format(e.to_string()))?;
        line.push('\n');
        self.file.write_all(line.as_bytes())?;
        self.file.sync_all()?; // before any signature is released
        self.lines += 1;
        Ok(())
    }

    /// Exports every record in the interchange format.
    #[must_use]
    pub fn export(&self, pubkey: &[u8]) -> Interchange {
        let entries = |kind: Kind| {
            self.records
                .iter()
                .filter(|(_, _, k, _)| *k == kind)
                .map(|(round, author, _, root)| Signed {
                    round: round.to_string(),
                    author: author.to_string(),
                    signing_root: to_hex(root),
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
                    let author = s
                        .author
                        .parse()
                        .map_err(|_| DbError::Format(format!("author {}", s.author)))?;
                    let record = Record {
                        kind,
                        round,
                        author,
                        root: from_hex(&s.signing_root)?,
                    };
                    if !self.records.contains(&(round, author, kind, record.root)) {
                        self.persist(record)?;
                        self.remember(record);
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
/// vertex/vote kinds in place of Ethereum's blocks/attestations. The `-2`
/// adds each slot's author (ADR-032).
pub const FORMAT: &str = "5-maya2c-2";

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
    /// The vertex's author, decimal.
    pub author: String,
    /// The vertex digest signed, hex.
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
