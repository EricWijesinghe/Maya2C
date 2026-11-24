//! `dev`, `fork` and `replay` wipe `--dir` on start. They may only wipe a
//! directory they made: a mistyped `--dir` must be refused, not deleted.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::{Block, BlockHeader};
use custom_l1_node::error::{NodeError, Result as NodeResult};
use custom_l1_node::state_pruner::snapshot::{BootstrapSource, SnapshotManifest};
use maya2c_cli::fork::{Source, copy_from};
use maya2c_cli::{SCRATCH_MARKER, reset_scratch_dir};

#[test]
fn a_directory_this_cli_did_not_make_is_refused_and_kept() {
    let work = tempfile::tempdir().unwrap();
    let precious = work.path().join("notes.txt");
    std::fs::write(&precious, "keep me").unwrap();

    let err = reset_scratch_dir(work.path()).unwrap_err();

    assert!(format!("{err:#}").contains("refusing"), "{err:#}");
    assert_eq!(std::fs::read_to_string(&precious).unwrap(), "keep me");
}

#[test]
fn a_marked_directory_is_wiped_and_re_marked() {
    let work = tempfile::tempdir().unwrap();
    let dir = work.path().join("dev");
    reset_scratch_dir(&dir).unwrap();
    std::fs::write(dir.join("old-chain.db"), "stale").unwrap();

    reset_scratch_dir(&dir).unwrap();

    assert!(!dir.join("old-chain.db").exists());
    assert!(dir.join(SCRATCH_MARKER).is_file());
}

#[test]
fn a_missing_or_empty_directory_is_accepted() {
    let work = tempfile::tempdir().unwrap();
    let missing = work.path().join("new");
    reset_scratch_dir(&missing).unwrap();
    assert!(missing.join(SCRATCH_MARKER).is_file());

    let empty = work.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    reset_scratch_dir(&empty).unwrap();
    assert!(empty.join(SCRATCH_MARKER).is_file());
}

#[test]
fn a_file_is_not_treated_as_a_directory() {
    let work = tempfile::tempdir().unwrap();
    let file = work.path().join("genesis.json");
    std::fs::write(&file, "{}").unwrap();

    assert!(reset_scratch_dir(&file).is_err());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "{}");
}

/// A source that is never reached: the guard must act before any fetch.
struct Unreachable;

impl BootstrapSource for Unreachable {
    fn tip_height(&self) -> NodeResult<u64> {
        Err(NodeError::Network("unreachable".into()))
    }
    fn headers(&self, _: u64, _: u64) -> NodeResult<Vec<BlockHeader>> {
        Err(NodeError::Network("unreachable".into()))
    }
    fn snapshot_manifest(&self) -> NodeResult<SnapshotManifest> {
        Err(NodeError::Network("unreachable".into()))
    }
    fn snapshot_chunk(&self, _: usize) -> NodeResult<Vec<u8>> {
        Err(NodeError::Network("unreachable".into()))
    }
    fn block(&self, _: u64) -> NodeResult<Block> {
        Err(NodeError::Network("unreachable".into()))
    }
}

#[test]
fn fork_refuses_to_wipe_an_unmarked_dir() {
    let work = tempfile::tempdir().unwrap();
    let precious = work.path().join("notes.txt");
    std::fs::write(&precious, "keep me").unwrap();
    let source = Source {
        url: String::new(),
        genesis: concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/site/public/testnet/genesis.json"
        )
        .into(),
        dir: work.path().to_path_buf(),
        snapshot_depth: None,
    };

    let err = copy_from(&source, None, &Unreachable)
        .err()
        .expect("an unmarked --dir must be refused");

    assert!(format!("{err:#}").contains("refusing"), "{err:#}");
    assert_eq!(std::fs::read_to_string(&precious).unwrap(), "keep me");
}
