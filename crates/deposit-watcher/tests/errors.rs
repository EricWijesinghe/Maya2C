//! Gap tests (2026-11-10 scan): a corrupt journal is an error and is never
//! read as an empty one (which would re-credit every deposit), and a failing
//! block source does not move the cursor.

#![allow(clippy::unwrap_used)]

use std::cell::Cell;
use std::collections::BTreeSet;

use maya_deposit_watcher::{BlockSource, Transfer, WatchError, Watcher};

fn watched() -> BTreeSet<String> {
    BTreeSet::from(["addr".to_string()])
}

/// Blocks 1..=top each carry one 10-unit deposit; `fail_at` errors that height.
struct Source {
    top: u64,
    fail_at: Cell<Option<u64>>,
}

impl BlockSource for Source {
    fn finalized_height(&self) -> u64 {
        self.top
    }
    fn transfers(&self, h: u64) -> Result<Vec<Transfer>, String> {
        if self.fail_at.get() == Some(h) {
            return Err(format!("node unreachable at {h}"));
        }
        Ok(vec![Transfer {
            txid: format!("tx{h}"),
            to: "addr".into(),
            amount: 10,
        }])
    }
}

#[test]
fn a_corrupt_complete_line_is_corrupt_not_empty_and_the_file_is_untouched() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("j.jsonl");
    let good = r#"{"height":1,"credits":[{"txid":"tx1","to":"addr","amount":10}]}"#;
    let text = format!("{good}\nnot json at all\n{good}\n");
    std::fs::write(&path, &text).unwrap();
    let Err(err) = Watcher::open(&path, watched()) else {
        panic!("a corrupt journal opened");
    };
    assert!(matches!(err, WatchError::Corrupt(2)), "{err}");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        text,
        "refusing must not truncate the evidence"
    );
}

#[test]
fn well_formed_json_of_the_wrong_shape_is_corrupt() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("j.jsonl");
    std::fs::write(&path, "{\"height\":\"one\",\"credits\":[]}\n").unwrap();
    assert!(matches!(
        Watcher::open(&path, watched()),
        Err(WatchError::Corrupt(1))
    ));
    std::fs::write(&path, "[]\n").unwrap();
    assert!(matches!(
        Watcher::open(&path, watched()),
        Err(WatchError::Corrupt(1))
    ));
}

#[test]
fn a_source_failure_leaves_the_cursor_and_the_journal_where_they_were() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("j.jsonl");
    let src = Source {
        top: 5,
        fail_at: Cell::new(Some(3)),
    };
    let mut w = Watcher::open(&path, watched()).unwrap();
    let err = w.poll(&src).unwrap_err();
    assert!(
        matches!(err, WatchError::Source(ref m) if m.contains("at 3")),
        "{err}"
    );
    // Blocks 1 and 2 are committed; 3 is not.
    assert_eq!(w.cursor(), 2);
    assert_eq!(w.balances()["addr"], 20);
    assert_eq!(w.credited(), 2);
    let len = std::fs::metadata(&path).unwrap().len();

    // Retrying while it still fails changes nothing.
    assert!(w.poll(&src).is_err());
    assert_eq!(
        (w.cursor(), std::fs::metadata(&path).unwrap().len()),
        (2, len)
    );

    // A restart sees the same cursor, then the source recovers and the
    // remaining blocks are credited once each.
    drop(w);
    let mut w = Watcher::open(&path, watched()).unwrap();
    assert_eq!(w.cursor(), 2);
    src.fail_at.set(None);
    assert_eq!(w.poll(&src).unwrap(), 3);
    assert_eq!((w.cursor(), w.credited(), w.balances()["addr"]), (5, 5, 50));
    assert_eq!(w.poll(&src).unwrap(), 0, "nothing is credited twice");
}
