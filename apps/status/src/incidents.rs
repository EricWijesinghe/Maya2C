//! The incident timeline: the log folded into one row per incident, and the
//! halt sync that drafts rows from the chain's own timestamps.
//!
//! A halt is drafted from [`Report`] (the same [`Timeline`](crate::Timeline)
//! answer the status page shows), never from the status service's clock for
//! the history. A draft is unsigned: it says what the chain showed, and waits
//! for an operator to publish it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::Report;
use crate::incident_store::{self as store, Entry, Kind, Record, Severity, StoreError};

/// One incident, as the timeline shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Incident {
    /// Stable id, e.g. `halt-1793172992`.
    pub id: String,
    /// True once an operator has signed a record for it.
    pub published: bool,
    /// How loud.
    pub severity: Severity,
    /// Start, Unix seconds.
    pub started: u64,
    /// End, Unix seconds; `None` while open.
    pub ended: Option<u64>,
    /// Headline.
    pub title: String,
    /// Detail.
    pub body: String,
    /// Link to the full report, https only.
    pub report: Option<String>,
    /// Log position of the record that set these fields.
    pub seq: u64,
    /// Hex public key of the publishing operator, if published.
    pub signer: Option<String>,
}

impl Incident {
    fn from_record(r: &Record, published: bool) -> Self {
        let b = &r.body;
        Self {
            id: b.id.clone(),
            published,
            severity: b.severity,
            started: b.started,
            ended: b.ended,
            title: b.title.clone(),
            body: b.body.clone(),
            report: b.report.clone(),
            seq: b.seq,
            signer: r.signer.clone(),
        }
    }
}

/// One row per incident, oldest start first. A publication's fields replace
/// the draft's; a later draft never replaces a published incident.
#[must_use]
pub fn fold(records: &[Record]) -> Vec<Incident> {
    let mut rows: BTreeMap<String, Incident> = BTreeMap::new();
    for r in records {
        let published_before = rows.get(&r.body.id).is_some_and(|i| i.published);
        match r.body.kind {
            Kind::Signed => {
                rows.insert(r.body.id.clone(), Incident::from_record(r, true));
            }
            Kind::Draft if !published_before => {
                rows.insert(r.body.id.clone(), Incident::from_record(r, false));
            }
            Kind::Draft => {}
        }
    }
    let mut out: Vec<Incident> = rows.into_values().collect();
    out.sort_by_key(|i| i.started);
    out
}

/// A halt as the chain shows it. `ended` is `None` while the chain is still halted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    /// The last block before the gap.
    pub height: u64,
    /// Its timestamp, Unix seconds.
    pub started: u64,
    /// The next block's timestamp, if the gap has closed.
    pub ended: Option<u64>,
}

/// The incident id of a halt: stable across restarts, because it is the
/// timestamp of the last block before the gap.
#[must_use]
pub fn span_id(started: u64) -> String {
    format!("halt-{started}")
}

/// Every halt since genesis, plus the one in progress, if any.
#[must_use]
pub fn spans_of(report: &Report) -> Vec<Span> {
    let mut spans: Vec<Span> = report
        .halts
        .iter()
        .map(|h| Span {
            height: h.height,
            started: h.started,
            ended: Some(h.ended),
        })
        .collect();
    if report.halted_now {
        spans.push(Span {
            height: report.height,
            started: report.last_block,
            ended: None,
        });
    }
    spans
}

fn span_title(s: &Span) -> String {
    match s.ended {
        Some(e) => format!("Chain halt, {} s", e.saturating_sub(s.started)),
        None => "Chain halted".to_owned(),
    }
}

fn span_body(s: &Span) -> String {
    match s.ended {
        Some(e) => format!(
            "No new block after height {}, from unix time {} to {e}.",
            s.height, s.started
        ),
        None => format!(
            "No new block after height {}, since unix time {}.",
            s.height, s.started
        ),
    }
}

/// The drafts that bring the log up to date with `spans`. Nothing is drafted
/// for a published incident, or for one already drafted with the same end.
#[must_use]
pub fn halt_drafts(existing: &[Incident], spans: &[Span], now: u64) -> Vec<Entry> {
    spans
        .iter()
        .filter_map(|s| {
            let id = span_id(s.started);
            match existing.iter().find(|i| i.id == id) {
                Some(i) if i.published || i.ended == s.ended => None,
                _ => Some(Entry {
                    kind: Kind::Draft,
                    id,
                    recorded_at: now,
                    started: s.started,
                    ended: s.ended,
                    severity: Severity::Critical,
                    title: span_title(s),
                    body: span_body(s),
                    report: None,
                }),
            }
        })
        .collect()
}

/// An incident log in one directory, with the operator keys that publish into it.
#[derive(Clone, Debug)]
pub struct Log {
    dir: PathBuf,
    trusted: Vec<Vec<u8>>,
}

impl Log {
    /// The log in `dir`, verified against `trusted` operator keys.
    #[must_use]
    pub fn new(dir: &Path, trusted: Vec<Vec<u8>>) -> Self {
        Self {
            dir: dir.to_path_buf(),
            trusted,
        }
    }

    /// Every record, verified.
    ///
    /// # Errors
    ///
    /// The log fails verification or cannot be read.
    pub fn records(&self) -> Result<Vec<Record>, StoreError> {
        store::load(&self.dir, &self.trusted)
    }

    /// The timeline rows, verified.
    ///
    /// # Errors
    ///
    /// As [`Log::records`].
    pub fn incidents(&self) -> Result<Vec<Incident>, StoreError> {
        Ok(fold(&self.records()?))
    }

    /// Drafts every halt the report shows that the log does not yet hold.
    /// Returns how many drafts were written.
    ///
    /// # Errors
    ///
    /// The log fails verification or cannot be written.
    pub fn sync_halts(&self, report: &Report, now: u64) -> Result<usize, StoreError> {
        let drafts = halt_drafts(&self.incidents()?, &spans_of(report), now);
        for d in &drafts {
            store::append(&self.dir, &self.trusted, d, None)?;
        }
        Ok(drafts.len())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::Halt;

    fn report(halts: Vec<Halt>, halted_now: bool, last_block: u64) -> Report {
        Report {
            height: 7,
            genesis: 0,
            last_block,
            uptime: 1.0,
            streak_seconds: 0,
            halted_now,
            since_last_block: 0,
            halts,
            threshold_seconds: 60,
        }
    }

    fn log() -> (tempfile::TempDir, Log) {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path(), Vec::new());
        (dir, log)
    }

    #[test]
    fn a_completed_halt_is_drafted_once_and_not_again() {
        let (_d, log) = log();
        let r = report(
            vec![Halt {
                height: 5,
                started: 1_000,
                ended: 1_120,
            }],
            false,
            1_120,
        );
        assert_eq!(log.sync_halts(&r, 2_000).unwrap(), 1);
        assert_eq!(
            log.sync_halts(&r, 2_001).unwrap(),
            0,
            "restart must not duplicate"
        );
        let rows = log.incidents().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "halt-1000");
        assert!(!rows[0].published);
        assert_eq!(rows[0].severity, Severity::Critical);
        assert_eq!(rows[0].ended, Some(1_120));
        assert_eq!(rows[0].title, "Chain halt, 120 s");
    }

    #[test]
    fn an_ongoing_halt_is_drafted_open_then_closed_in_place() {
        let (_d, log) = log();
        let open = report(vec![], true, 1_000);
        assert_eq!(log.sync_halts(&open, 1_090).unwrap(), 1);
        let rows = log.incidents().unwrap();
        assert_eq!(rows[0].ended, None);
        assert_eq!(rows[0].title, "Chain halted");

        let closed = report(
            vec![Halt {
                height: 7,
                started: 1_000,
                ended: 1_150,
            }],
            false,
            1_150,
        );
        assert_eq!(
            log.sync_halts(&closed, 1_151).unwrap(),
            1,
            "the end is a new draft line"
        );
        let rows = log.incidents().unwrap();
        assert_eq!(rows.len(), 1, "one incident, not two");
        assert_eq!(rows[0].ended, Some(1_150));
        assert_eq!(
            log.records().unwrap().len(),
            2,
            "append-only: both lines remain"
        );
    }

    #[test]
    fn a_published_incident_is_never_overwritten_by_a_draft() {
        let (_d, log) = log();
        let r = report(
            vec![Halt {
                height: 5,
                started: 1_000,
                ended: 1_120,
            }],
            false,
            1_120,
        );
        log.sync_halts(&r, 2_000).unwrap();
        let op = crate::operator::Operator::from_seed([3; 32]);
        let log = Log::new(log.dir.as_path(), vec![op.public_key().to_vec()]);
        let publish = Entry {
            kind: Kind::Signed,
            id: "halt-1000".into(),
            recorded_at: 3_000,
            started: 1_000,
            ended: Some(1_120),
            severity: Severity::Critical,
            title: "Halt from a bad validator".into(),
            body: "Operator's account of the halt.".into(),
            report: Some("https://status.maya2c.dev/reports/halt-1000".into()),
        };
        store::append(&log.dir, &log.trusted, &publish, Some(&op)).unwrap();
        // A later draft for the same halt must not change a published row.
        let drafts = halt_drafts(
            &log.incidents().unwrap(),
            &[Span {
                height: 5,
                started: 1_000,
                ended: Some(1_999),
            }],
            4_000,
        );
        assert!(drafts.is_empty());
        let rows = log.incidents().unwrap();
        assert!(rows[0].published);
        assert_eq!(rows[0].title, "Halt from a bad validator");
        assert_eq!(
            rows[0].signer.as_deref(),
            Some(hex::encode(op.public_key()).as_str())
        );
    }

    #[test]
    fn spans_cover_past_halts_and_the_one_in_progress() {
        let r = report(
            vec![Halt {
                height: 2,
                started: 10,
                ended: 20,
            }],
            true,
            30,
        );
        let spans = spans_of(&r);
        assert_eq!(spans.len(), 2);
        assert_eq!(
            spans[1],
            Span {
                height: 7,
                started: 30,
                ended: None
            }
        );
        assert_eq!(span_id(30), "halt-30");
    }

    #[test]
    fn a_tampered_log_refuses_to_sync() {
        let (d, log) = log();
        log.sync_halts(&report(vec![], true, 1_000), 1_100).unwrap();
        let path = d.path().join(store::FILE);
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replace("Chain halted", "All quiet")).unwrap();
        assert!(log.sync_halts(&report(vec![], true, 1_000), 1_200).is_err());
    }
}
