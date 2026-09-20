//! Pruned nodes: bootstrapping from a snapshot, archiving, pruning, fetching
//! cold blocks back, and the horizon a pruned node will not reorg below.
//!
//! Every test runs at a depth of K = 20 blocks instead of the production
//! 30,000, through `PruneConfig`, so a whole lifecycle fits in seconds.
//!
//! The central claim is test 1: a node bootstraps to the same tip and the same
//! state root as an archive node, while a recording source proves it never
//! asked for a single body below the snapshot height.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use custom_l1_node::consensus::chain::{Chain, ChainConfig, InsertOutcome};
use custom_l1_node::core::TxKind;
use custom_l1_node::core::block::{Block, BlockHeader};
use custom_l1_node::core::payload::{ContractCall, ContractDeploy};
use custom_l1_node::core::transaction::{Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::{NodeError, Result};
use custom_l1_node::state::account::Account;
use custom_l1_node::state::db::StateDB;
use custom_l1_node::state_pruner::archive::{Archiver, prune_round};
use custom_l1_node::state_pruner::cold::ColdBlocks;
use custom_l1_node::state_pruner::snapshot::{
    BootstrapSource, CHUNK_ENTRIES, SnapshotManifest, Snapshots, bootstrap_pruned, chunk_hash,
    decode_chunk, encode_chunk,
};
use custom_l1_node::state_pruner::{ArchivePolicy, PruneConfig};
use maya_archive::{ArchiveError, ArchiveStore, LocalDirStore, Locator};
use tempfile::TempDir;

/// The test depth, standing in for one DAG epoch.
const K: u64 = 20;

/// A snapshot every this many blocks.
const SNAPSHOT_INTERVAL: u64 = 10;

/// A contract that writes key `k` = 1234 when called.
const WRITER_WAT: &str = r#"(module
    (import "env" "storage_write" (func $write (param i32 i32 i32 i32)))
    (memory (export "memory") 1)
    (func (export "invoke") (param i32) (result i64)
      (i32.store8 (i32.const 0) (i32.const 107))
      (i32.store (i32.const 16) (i32.const 1234))
      (call $write (i32.const 0) (i32.const 1) (i32.const 16) (i32.const 4))
      (i64.const 0)))"#;

fn prune_config() -> PruneConfig {
    PruneConfig {
        depth: K,
        batch: 10,
        archive: ArchivePolicy::Required,
    }
}

fn genesis() -> Block {
    Block::new(
        BlockHeader {
            prev_hash: [0u8; 32],
            state_root: [0u8; 32],
            timestamp: 1_000_000,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        Vec::new(),
    )
}

fn config() -> ChainConfig {
    ChainConfig::without_pow_verification()
}

fn signed(kind: TxKind, nonce: u64, key: &HybridSigningKey) -> Transaction {
    let mut tx = Transaction::with_kind(kind, nonce);
    tx.sign(key).expect("sign");
    tx
}

fn transfer(key: &HybridSigningKey, amount: u64, nonce: u64) -> Transaction {
    let mut tx = Transaction::new(
        vec![],
        vec![TxOutput {
            amount,
            recipient: [0x42; 32],
        }],
        nonce,
    );
    tx.sign(key).expect("sign");
    tx
}

/// A node that keeps everything, mines, and takes snapshots.
struct ArchiveNode {
    chain: Chain,
    key: HybridSigningKey,
    nonce: u64,
    snapshots: Snapshots,
    _dirs: (TempDir, TempDir),
}

impl ArchiveNode {
    fn new(key: HybridSigningKey) -> Self {
        let state_dir = TempDir::new().expect("dir");
        let snapshot_dir = TempDir::new().expect("dir");
        let state = Arc::new(StateDB::open(state_dir.path()).expect("state"));
        state
            .put_account(
                &key.address(),
                &Account {
                    balance: 1_000_000_000,
                    nonce: 0,
                },
            )
            .expect("fund");
        Self {
            chain: Chain::open(state, genesis(), config()).expect("chain"),
            key,
            nonce: 0,
            snapshots: Snapshots::new(snapshot_dir.path()).expect("snapshots"),
            _dirs: (state_dir, snapshot_dir),
        }
    }

    /// A node with the same genesis state that has followed `other` to
    /// `height`, and will mine its own branch from there.
    fn following(other: &ArchiveNode, height: u64) -> Self {
        let mut node = Self::new(other.key.clone());
        for h in 1..=height {
            node.chain.insert_block(other.block(h)).expect("follow");
        }
        // Its own state's nonce: the other node may have mined past `height`.
        node.nonce = node
            .chain
            .state()
            .get_account(&node.key.address())
            .expect("account")
            .nonce;
        node
    }

    fn block(&self, height: u64) -> Block {
        let state = self.chain.state();
        let id = state
            .canonical_id(height)
            .expect("read")
            .expect("canonical");
        state.load_block(&id).expect("read").expect("body")
    }

    /// Mines `count` blocks. The first few carry a transfer each, block 3
    /// deploys a contract and block 4 writes its storage, so the snapshot has
    /// accounts, contract code and contract storage in it.
    fn mine(&mut self, count: u64, salt: u64) {
        for _ in 0..count {
            let height = self.chain.height() + 1;
            let transactions = match height {
                3 => vec![signed(
                    TxKind::DeployContract(ContractDeploy {
                        code: wat::parse_str(WRITER_WAT).expect("wat"),
                    }),
                    self.nonce,
                    &self.key,
                )],
                4 => {
                    let contract = custom_l1_node::core::payload::derive_contract_id(
                        &self.key.address(),
                        self.nonce - 1,
                        &wat::parse_str(WRITER_WAT).expect("wat"),
                    );
                    vec![signed(
                        TxKind::CallContract(ContractCall {
                            contract,
                            input: Vec::new(),
                            gas_limit: 1_000_000,
                        }),
                        self.nonce,
                        &self.key,
                    )]
                }
                h if h <= 12 || h % 7 == 0 => vec![transfer(&self.key, 5 + salt, self.nonce)],
                _ => Vec::new(),
            };
            self.nonce += transactions.len() as u64;
            let timestamp = 1_000_000 + height * 15 + salt;
            let block = self
                .chain
                .candidate_block(timestamp, transactions)
                .expect("candidate");
            assert!(matches!(
                self.chain.insert_block(block).expect("insert"),
                InsertOutcome::Extended { .. }
            ));
            if height.is_multiple_of(SNAPSHOT_INTERVAL) {
                self.snapshots
                    .take(self.chain.state(), height, &self.chain.tip())
                    .expect("snapshot");
            }
        }
    }
}

type Tamper = Box<dyn Fn(&mut Vec<(Vec<u8>, Vec<u8>)>)>;

/// An archive node as a bootstrap source, recording every body asked for and
/// optionally lying consistently about its snapshot.
struct Source<'a> {
    node: &'a ArchiveNode,
    bodies: RefCell<Vec<u64>>,
    tamper: Option<Tamper>,
    wrong_block: bool,
}

impl<'a> Source<'a> {
    fn honest(node: &'a ArchiveNode) -> Self {
        Self {
            node,
            bodies: RefCell::default(),
            tamper: None,
            wrong_block: false,
        }
    }

    /// The snapshot as served, re-chunked and re-hashed after tampering, so the
    /// only thing that can catch the lie is the state root.
    fn served(&self) -> Result<(SnapshotManifest, Vec<Vec<u8>>)> {
        let tip = self.node.chain.height();
        let height = self
            .node
            .snapshots
            .serveable(tip, K)?
            .ok_or_else(|| NodeError::Network("no snapshot".into()))?;
        let (mut manifest, chunks) = self.node.snapshots.load(height)?;
        if self.wrong_block {
            manifest.block_id = [0xAB; 32];
        }
        let Some(tamper) = &self.tamper else {
            return Ok((manifest, chunks));
        };
        let mut entries: Vec<(Vec<u8>, Vec<u8>)> = chunks
            .iter()
            .flat_map(|chunk| decode_chunk(chunk).expect("chunk"))
            .collect();
        tamper(&mut entries);
        let chunks: Vec<Vec<u8>> = entries.chunks(CHUNK_ENTRIES).map(encode_chunk).collect();
        manifest.chunks = chunks.iter().map(|chunk| chunk_hash(chunk)).collect();
        Ok((manifest, chunks))
    }
}

impl BootstrapSource for Source<'_> {
    fn tip_height(&self) -> Result<u64> {
        Ok(self.node.chain.height())
    }

    fn headers(&self, from: u64, to: u64) -> Result<Vec<BlockHeader>> {
        (from..=to).map(|h| Ok(self.node.block(h).header)).collect()
    }

    fn snapshot_manifest(&self) -> Result<SnapshotManifest> {
        Ok(self.served()?.0)
    }

    fn snapshot_chunk(&self, index: usize) -> Result<Vec<u8>> {
        Ok(self.served()?.1[index].clone())
    }

    fn block(&self, height: u64) -> Result<Block> {
        self.bodies.borrow_mut().push(height);
        Ok(self.node.block(height))
    }
}

/// A fresh, empty database.
fn empty_state() -> (Arc<StateDB>, TempDir) {
    let dir = TempDir::new().expect("dir");
    (Arc::new(StateDB::open(dir.path()).expect("state")), dir)
}

fn archive_node(blocks: u64) -> ArchiveNode {
    let mut node = ArchiveNode::new(generate_signing_key().expect("key"));
    node.mine(blocks, 0);
    node
}

// ---------------------------------------------------------------------------
// 1. bootstrapping without history
// ---------------------------------------------------------------------------

#[test]
fn a_pruned_node_bootstraps_without_a_single_body_below_the_snapshot() {
    let archive = archive_node(70);
    let source = Source::honest(&archive);
    let (state, _dir) = empty_state();

    let pruned =
        bootstrap_pruned(Arc::clone(&state), genesis(), config(), &source, K).expect("bootstrap");

    // The newest snapshot at least K below tip 70 is the one at 50.
    assert_eq!(pruned.prune_horizon(), 50);
    assert_eq!(pruned.tip(), archive.chain.tip());
    assert_eq!(
        state.state_root().expect("root"),
        archive.chain.state().state_root().expect("root"),
        "the pruned node must hold exactly the archive node's state"
    );
    assert_eq!(
        *source.bodies.borrow(),
        (51..=70).collect::<Vec<u64>>(),
        "no body at or below the snapshot height may be downloaded"
    );
    let early = state.canonical_id(30).expect("read").expect("header kept");
    assert!(
        !state.has_body(&early).expect("read"),
        "held a body it never fetched"
    );
    assert_eq!(state.uncovered_keys().expect("scan"), Vec::<Vec<u8>>::new());
}

// ---------------------------------------------------------------------------
// 2. a lying snapshot
// ---------------------------------------------------------------------------

fn refuses(tamper: Tamper) {
    let archive = archive_node(70);
    let mut source = Source::honest(&archive);
    source.tamper = Some(tamper);
    let (state, _dir) = empty_state();

    let result = bootstrap_pruned(Arc::clone(&state), genesis(), config(), &source, K);
    assert!(
        matches!(result, Err(NodeError::StateRootMismatch { .. })),
        "a consistent lie must be caught by the state root: {:?}",
        result.err()
    );
    assert!(
        state.committed_entries().expect("scan").is_empty()
            && state.chain_meta().expect("read").is_none(),
        "a refused bootstrap must leave nothing behind, so it can be retried"
    );
    // And a retry against an honest source then succeeds on the same database.
    let honest = Source::honest(&archive);
    bootstrap_pruned(state, genesis(), config(), &honest, K).expect("retry");
}

#[test]
fn a_snapshot_with_one_changed_balance_is_refused() {
    refuses(Box::new(|entries| {
        let account = entries
            .iter_mut()
            .find(|(key, _)| key.starts_with(b"acct:"))
            .expect("an account");
        account.1[0] ^= 1;
    }));
}

#[test]
fn a_snapshot_with_a_changed_contract_slot_is_refused() {
    refuses(Box::new(|entries| {
        let slot = entries
            .iter_mut()
            .find(|(key, _)| key.starts_with(b"cstate:"))
            .expect("the contract wrote a slot");
        slot.1[0] ^= 1;
    }));
}

#[test]
fn a_snapshot_missing_contract_code_is_refused() {
    refuses(Box::new(|entries| {
        entries.retain(|(key, _)| !key.starts_with(b"code:"));
    }));
}

#[test]
fn a_snapshot_off_the_best_chain_or_too_shallow_is_refused() {
    let archive = archive_node(70);

    let mut wrong = Source::honest(&archive);
    wrong.wrong_block = true;
    let (state, _dir) = empty_state();
    assert!(matches!(
        bootstrap_pruned(state, genesis(), config(), &wrong, K),
        Err(NodeError::Network(_))
    ));

    // Asking for more depth than the served snapshot has.
    let (state, _dir) = empty_state();
    assert!(matches!(
        bootstrap_pruned(state, genesis(), config(), &Source::honest(&archive), 30),
        Err(NodeError::Network(_))
    ));
}

#[test]
fn a_database_that_is_not_empty_is_refused() {
    let archive = archive_node(30);
    let (state, _dir) = empty_state();
    state
        .put_account(
            &[1; 32],
            &Account {
                balance: 1,
                nonce: 0,
            },
        )
        .expect("put");
    assert!(matches!(
        bootstrap_pruned(state, genesis(), config(), &Source::honest(&archive), K),
        Err(NodeError::Storage(_))
    ));
}

// ---------------------------------------------------------------------------
// 3. archive, prune, fetch back
// ---------------------------------------------------------------------------

/// A store that accepts nothing, to prove nothing is pruned on failure.
struct BrokenStore;

impl ArchiveStore for BrokenStore {
    fn kind(&self) -> &'static str {
        "broken"
    }

    fn put(&self, _: &maya_archive::Archive) -> maya_archive::Result<Locator> {
        Err(ArchiveError::Http("the store is down".into()))
    }

    fn get(&self, _: &Locator, _: &maya_archive::Cid) -> maya_archive::Result<Vec<u8>> {
        Err(ArchiveError::Http("the store is down".into()))
    }
}

/// Bootstraps a pruned node from an archive node at `blocks`, then follows
/// the archive node for `more` blocks.
fn pruned_follower(archive: &mut ArchiveNode, more: u64) -> (Mutex<Chain>, TempDir) {
    let (state, dir) = empty_state();
    let pruned = bootstrap_pruned(state, genesis(), config(), &Source::honest(archive), K)
        .expect("bootstrap");
    let first = archive.chain.height() + 1;
    archive.mine(more, 0);
    let pruned = Mutex::new(pruned);
    for height in first..=archive.chain.height() {
        pruned
            .lock()
            .expect("lock")
            .insert_block(archive.block(height))
            .expect("follow");
    }
    (pruned, dir)
}

#[test]
fn nothing_is_pruned_when_the_archive_fails() {
    let mut archive = archive_node(70);
    let (pruned, _dir) = pruned_follower(&mut archive, 30);
    let archiver = Archiver::new("maya-test", vec![Box::new(BrokenStore)]);

    assert!(prune_round(&pruned, &prune_config(), Some(&archiver)).is_err());
    let chain = pruned.lock().expect("lock");
    assert_eq!(
        chain.prune_horizon(),
        50,
        "the horizon moved without an archive"
    );
    let id = chain.state().canonical_id(51).expect("read").expect("id");
    assert!(
        chain.state().has_body(&id).expect("read"),
        "a body was deleted"
    );
}

#[test]
fn a_pruned_node_archives_prunes_and_serves_verified_cold_blocks() {
    let mut archive = archive_node(70);
    let (pruned, _dir) = pruned_follower(&mut archive, 30);
    let archive_dir = TempDir::new().expect("dir");
    let archiver = Archiver::new(
        "maya-test",
        vec![Box::new(
            LocalDirStore::new(archive_dir.path()).expect("store"),
        )],
    );

    // Tip 100, horizon 50, depth 20: three batches of ten, then nothing.
    for expected in [51..=60, 61..=70, 71..=80] {
        let range = prune_round(&pruned, &prune_config(), Some(&archiver)).expect("round");
        assert_eq!(range, Some(expected));
    }
    assert_eq!(
        prune_round(&pruned, &prune_config(), Some(&archiver)).expect("round"),
        None
    );

    let chain = pruned.lock().expect("lock");
    let state = chain.state();
    assert_eq!(chain.prune_horizon(), 80);
    let id = state.canonical_id(65).expect("read").expect("id");
    assert!(
        !state.has_body(&id).expect("read"),
        "the body was not pruned"
    );
    assert!(
        state.stored_header(&id).expect("read").is_some(),
        "the header must stay"
    );

    let cold = ColdBlocks::new(vec![Box::new(
        LocalDirStore::new(archive_dir.path()).expect("store"),
    )]);
    assert_eq!(
        cold.fetch(state, 65).expect("cold fetch"),
        archive.block(65)
    );
    // Below the snapshot there was never a body, so there is no archive.
    assert!(cold.fetch(state, 40).is_err());

    // Replace the stored archive with another batch's: the root the receipt
    // recorded no longer matches, and the copy is refused.
    let receipt = state.receipt_for(65).expect("read").expect("receipt");
    let other = maya_archive::build_archive(
        "maya-test",
        &chain.canonical_batch(&(81..=90)).expect("batch"),
    )
    .expect("build");
    let file = archive_dir.path().join(format!("{}.car.zst", receipt.root));
    std::fs::write(&file, maya_archive::compress(&other.car).expect("compress")).expect("write");
    assert!(
        cold.fetch(state, 65).is_err(),
        "a substituted archive was accepted"
    );
}

// ---------------------------------------------------------------------------
// 4. the horizon
// ---------------------------------------------------------------------------

#[test]
fn a_reorg_below_the_horizon_is_refused_and_one_above_it_lands() {
    let mut archive = archive_node(70);
    let (pruned, _dir) = pruned_follower(&mut archive, 30);
    let archiver = Archiver::new(
        "maya-test",
        vec![Box::new(
            LocalDirStore::new(TempDir::new().expect("dir").keep()).expect("store"),
        )],
    );
    while prune_round(&pruned, &prune_config(), Some(&archiver))
        .expect("round")
        .is_some()
    {}
    assert_eq!(pruned.lock().expect("lock").prune_horizon(), 80);

    // A heavier branch forking at 70, below the horizon: refused at its first
    // block, and nothing moves.
    let mut deep = ArchiveNode::following(&archive, 70);
    deep.mine(40, 1);
    let tip_before = pruned.lock().expect("lock").tip();
    assert!(matches!(
        pruned.lock().expect("lock").insert_block(deep.block(71)),
        Err(NodeError::BelowPruneHorizon { .. })
    ));
    assert_eq!(pruned.lock().expect("lock").tip(), tip_before);

    // A heavier branch forking at 90, above it: an ordinary reorg.
    let mut shallow = ArchiveNode::following(&archive, 90);
    shallow.mine(15, 2);
    let mut chain = pruned.lock().expect("lock");
    for height in 91..=shallow.chain.height() {
        chain.insert_block(shallow.block(height)).expect("reorg");
    }
    assert_eq!(chain.tip(), shallow.chain.tip());
    assert_eq!(
        chain.state().state_root().expect("root"),
        shallow.chain.state().state_root().expect("root")
    );
}

#[test]
fn restarting_a_pruned_node_keeps_its_horizon_and_receipts() {
    let mut archive = archive_node(70);
    let (pruned, dir) = pruned_follower(&mut archive, 30);
    let archive_dir = TempDir::new().expect("dir");
    let archiver = Archiver::new(
        "maya-test",
        vec![Box::new(
            LocalDirStore::new(archive_dir.path()).expect("store"),
        )],
    );
    prune_round(&pruned, &prune_config(), Some(&archiver)).expect("round");
    let (tip, horizon) = {
        let chain = pruned.lock().expect("lock");
        (chain.tip(), chain.prune_horizon())
    };
    drop(pruned);

    let state = Arc::new(StateDB::open(dir.path()).expect("reopen"));
    let chain = Chain::open(Arc::clone(&state), genesis(), config()).expect("reopen chain");
    assert_eq!(chain.tip(), tip);
    assert_eq!(chain.prune_horizon(), horizon);
    assert!(state.receipt_for(55).expect("read").is_some());
    let mut counts = BTreeMap::new();
    for (key, _) in state.committed_entries().expect("scan") {
        *counts.entry(key[..4].to_vec()).or_insert(0) += 1;
    }
    assert!(!counts.is_empty());
}
