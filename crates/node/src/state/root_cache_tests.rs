//! The root cache against the full rebuild (ADR-062a).
//!
//! The authority is `sparse::root` over every committed account, the function
//! `StateDB::state_root` has always used. Every test here asserts the cache
//! agrees with it: the root, the account count, and every stored branch.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use maya_stateless_core::{Blake3, sparse};
use primitive_types::U256;
use rand_chacha::ChaCha20Rng;
use rand_chacha::rand_core::{RngCore, SeedableRng};
use rocksdb::WriteBatch;
use tempfile::TempDir;

use super::{
    Change, MARKER, NODES_END, ROOT_CACHE_PREFIX, RootCacheStats, UNCHECKED, Walk, marker,
    node_key, root_after, touches_accounts,
};
use crate::state::account::{Account, Address};
use crate::state::commitments::{LOCAL_ONLY_PREFIXES, is_committed};
use crate::state::db::{Overlay, StateDB};
use crate::state::stateless::SPARSE_ACCOUNTS_KEY;

fn sparse_db(dir: &TempDir) -> StateDB {
    let db = StateDB::open(dir.path()).expect("open");
    db.raw_put_for_test(SPARSE_ACCOUNTS_KEY, &[1])
        .expect("mark");
    db
}

fn leaves(db: &StateDB) -> Vec<(Address, [u8; 40])> {
    db.all_accounts()
        .expect("accounts")
        .iter()
        .map(|(address, account)| (*address, account.encode()))
        .collect()
}

fn full_root(db: &StateDB) -> [u8; 32] {
    sparse::root(&Blake3, &leaves(db)).expect("sorted")
}

/// Every stored branch, without the marker.
fn stored_nodes(db: &StateDB) -> BTreeMap<Vec<u8>, Vec<u8>> {
    db.scan_prefix(ROOT_CACHE_PREFIX)
        .expect("scan")
        .into_iter()
        .filter(|(key, _)| key.as_slice() != MARKER)
        .collect()
}

/// The branches a from-scratch build over committed accounts stores.
fn rebuilt_nodes(db: &StateDB) -> BTreeMap<Vec<u8>, Vec<u8>> {
    let snapshot = db.raw_db().snapshot();
    let mut walk = Walk::new(&snapshot);
    walk.build(&leaves(db));
    walk.delta
        .into_iter()
        .map(|(key, children)| (key.to_vec(), children.unwrap().to_vec()))
        .collect()
}

/// The cache is valid and equal, branch for branch, to a rebuild.
fn assert_exact(db: &StateDB) {
    let accounts = u64::try_from(leaves(db).len()).unwrap();
    assert_eq!(
        marker(db.raw_db()).unwrap(),
        Some((full_root(db), accounts))
    );
    assert_eq!(stored_nodes(db), rebuilt_nodes(db));
}

fn account(rng: &mut ChaCha20Rng) -> Account {
    Account {
        balance: U256::from(rng.next_u64() % 1_000_000),
        nonce: rng.next_u64() % 8,
    }
}

/// An address that is often a neighbour of one already known: one bit away,
/// so paths share up to 255 bits and the deep, collapsing cases are hit.
fn address(rng: &mut ChaCha20Rng, known: &[Address]) -> Address {
    let mut out = [0; 32];
    match (rng.next_u64() % 3, known.is_empty()) {
        (0, false) => {
            out = known[pick(rng, known.len())];
            let bit = pick(rng, 256);
            out[bit / 8] ^= 0x80 >> (bit % 8);
        }
        (1, false) => out = known[pick(rng, known.len())],
        _ => rng.fill_bytes(&mut out),
    }
    out
}

fn pick(rng: &mut ChaCha20Rng, below: usize) -> usize {
    let below = u64::try_from(below).expect("fits");
    usize::try_from(rng.next_u64() % below).expect("fits")
}

fn random_overlay(rng: &mut ChaCha20Rng, known: &mut Vec<Address>) -> Overlay {
    let mut overlay = Overlay::new();
    for _ in 0..=rng.next_u64() % 12 {
        let address = address(rng, known);
        overlay.accounts.insert(address, account(rng));
        known.push(address);
    }
    overlay
}

/// Previews then commits `overlay` under `id`, as a block does: the commit
/// stages the plan the preview made.
fn commit(db: &StateDB, mut overlay: Overlay, id: &[u8; 32]) -> [u8; 32] {
    let predicted = db.root_with_overlay(&mut overlay).expect("preview");
    db.commit_journaled(&overlay, id, |_| ()).expect("commit");
    predicted
}

#[test]
fn the_cache_prefix_is_local_only_and_outside_the_root() {
    assert!(!is_committed(ROOT_CACHE_PREFIX));
    assert!(!is_committed(&node_key(0, &[0; 32])));
    assert!(LOCAL_ONLY_PREFIXES.contains(&ROOT_CACHE_PREFIX));
    // Node keys sit between the marker and the end bound, and only there.
    assert!(MARKER < node_key(0, &[0; 32]).as_slice());
    assert!(node_key(255, &[0xFF; 32]).as_slice() < NODES_END);
    // A node sorts before everything below it, so the topmost branch under a
    // prefix is the first key from it.
    let below = node_key(9, &[0x80; 32]);
    assert!(node_key(1, &[0x80; 32]) < below);
    assert!(node_key(0, &[0; 32]) < below);
}

#[test]
fn incremental_root_equals_full_rebuild_over_random_histories() {
    for seed in 0..12 {
        let dir = TempDir::new().expect("tempdir");
        let db = sparse_db(&dir);
        let mut rng = ChaCha20Rng::seed_from_u64(seed);
        let mut known = Vec::new();
        let mut journal: Vec<[u8; 32]> = Vec::new();
        // No cache yet, so the first commit builds one.
        let mut cache_withdrawn = true;

        for step in 0_u64..80 {
            let before = db.root_cache_stats();
            match rng.next_u64() % 10 {
                // A reorg: undo the newest block, deleting accounts it created.
                0..=2 if !journal.is_empty() => {
                    db.revert_block(&journal.pop().unwrap()).expect("revert");
                }
                // A write outside every commit path withdraws the cache; the
                // next commit rebuilds it.
                3 if step % 7 == 0 => {
                    let address = address(&mut rng, &known);
                    db.put_account(&address, &account(&mut rng)).expect("put");
                    known.push(address);
                    assert_eq!(marker(db.raw_db()).unwrap(), None);
                    cache_withdrawn = true;
                    continue;
                }
                _ => {
                    let overlay = random_overlay(&mut rng, &mut known);
                    let id = *blake3::hash(&[seed.to_le_bytes(), step.to_le_bytes()].concat())
                        .as_bytes();
                    let predicted = commit(&db, overlay, &id);
                    journal.push(id);
                    // The preview, from the cache, is the committed full root.
                    assert_eq!(
                        predicted,
                        db.state_root().expect("root"),
                        "seed {seed} step {step}"
                    );
                }
            }
            // One plan a step, from the cache unless it had been withdrawn:
            // a walk that silently fell back to a rebuild fails here, on
            // reverts as on commits.
            let after = db.root_cache_stats();
            let (walked, built) = if cache_withdrawn { (0, 1) } else { (1, 0) };
            assert_eq!(
                after,
                RootCacheStats {
                    walked: before.walked + walked,
                    built: before.built + built,
                    repaired: 0,
                },
                "seed {seed} step {step}"
            );
            cache_withdrawn = false;
            assert_exact(&db);
        }
        // Unwinding everything returns to the cache of the starting state.
        while let Some(id) = journal.pop() {
            db.revert_block(&id).expect("revert");
            assert_eq!(
                marker(db.raw_db()).unwrap().map(|(root, _)| root),
                Some(full_root(&db))
            );
        }
        assert_exact(&db);
        assert_eq!(db.root_cache_stats().repaired, 0);
    }
}

#[test]
fn a_dense_post_state_withdraws_the_cache() {
    let dir = TempDir::new().expect("tempdir");
    let db = sparse_db(&dir);
    let mut rng = ChaCha20Rng::seed_from_u64(99);
    let mut known = Vec::new();
    commit(&db, random_overlay(&mut rng, &mut known), &[1; 32]);
    assert_exact(&db);

    // A block that removes the `sl:` marker, as a revert past activation does.
    let mut overlay = random_overlay(&mut rng, &mut known);
    overlay.records.insert(SPARSE_ACCOUNTS_KEY.to_vec(), None);
    commit(&db, overlay, &[2; 32]);
    assert_eq!(marker(db.raw_db()).unwrap(), None);

    // Undoing it restores sparse accounts, and the cache with them.
    db.revert_block(&[2; 32]).expect("revert");
    assert_exact(&db);
}

#[test]
fn the_cache_survives_a_restart_and_a_damaged_one_is_rebuilt_on_open() {
    let dir = TempDir::new().expect("tempdir");
    let mut rng = ChaCha20Rng::seed_from_u64(7);
    let mut known = Vec::new();
    let root = {
        let db = sparse_db(&dir);
        for step in 0..20_u8 {
            commit(&db, random_overlay(&mut rng, &mut known), &[step; 32]);
        }
        db.state_root().expect("root")
    };

    // A clean restart: the cache is still valid and still exact.
    let db = StateDB::open(dir.path()).expect("reopen");
    assert!(db.verify_root_cache().expect("verify"));
    assert_exact(&db);
    assert_eq!(db.state_root().expect("root"), root);
    assert_eq!(db.root_cache_stats().repaired, 0);

    // One branch damaged on disk, with the marker still right: the shallow
    // check would pass it, the branch-by-branch check does not.
    let (key, _) = stored_nodes(&db).into_iter().next_back().expect("a node");
    db.raw_put_for_test(&key, &[0xAB; 64]).expect("damage");
    drop(db);
    let db = StateDB::open(dir.path()).expect("reopen");
    assert!(!db.verify_root_cache().expect("verify"));
    assert_exact(&db);
    assert_eq!(db.root_cache_stats().repaired, 1);

    // A stray branch no build would write is caught the same way.
    db.raw_put_for_test(&node_key(200, &[0x5A; 32]), &[1; 64])
        .expect("stray");
    assert!(!db.verify_root_cache().expect("verify"));
    assert_exact(&db);
    assert_eq!(db.root_cache_stats().repaired, 2);
}

#[test]
fn a_wrong_cache_costs_a_rebuild_not_the_block() {
    let dir = TempDir::new().expect("tempdir");
    let db = sparse_db(&dir);
    let mut rng = ChaCha20Rng::seed_from_u64(11);
    let mut known = Vec::new();
    for step in 0..8_u8 {
        commit(&db, random_overlay(&mut rng, &mut known), &[step; 32]);
    }
    assert_exact(&db);

    // Damage every branch's children: the cache still looks valid, and a
    // root it gives over any untouched sibling is wrong. The debug
    // cross-check would stop the test here, so it is off for this one.
    UNCHECKED.with(|unchecked| unchecked.set(true));
    for (key, _) in stored_nodes(&db) {
        db.raw_put_for_test(&key, &[0xCD; 64]).expect("damage");
    }

    // One new account: every other path is an untouched, damaged sibling.
    let mut overlay = Overlay::new();
    let mut fresh = [0; 32];
    rng.fill_bytes(&mut fresh);
    overlay.accounts.insert(fresh, account(&mut rng));
    let from_cache = db.root_with_overlay(&mut overlay).expect("preview");
    let (full, _) = db.root_from(&overlay, false).expect("full");
    assert_ne!(from_cache, full, "the damage reaches the root");

    // A block declaring some third root is still refused.
    let mut refused = overlay.clone();
    assert!(
        db.check_declared_root(&mut refused, from_cache, &[0x11; 32])
            .is_err()
    );
    assert_eq!(db.root_cache_stats().repaired, 0);

    // A block declaring the full rebuild's root is accepted on it, and the
    // cache is withdrawn, so the commit rebuilds it.
    let accepted = db
        .check_declared_root(&mut overlay, from_cache, &full)
        .expect("accepted on the full rebuild");
    assert_eq!(accepted, full);
    assert_eq!(marker(db.raw_db()).unwrap(), None);
    assert_eq!(db.root_cache_stats().repaired, 1);
    db.commit_journaled(&overlay, &[0x77; 32], |_| ())
        .expect("commit");
    assert_eq!(db.state_root().expect("root"), full);
    assert_exact(&db);
    UNCHECKED.with(|unchecked| unchecked.set(false));
}

#[test]
fn every_write_that_can_touch_accounts_withdraws_the_cache() {
    let mut batch = WriteBatch::default();
    batch.put(b"d:other", b"1");
    assert!(
        !touches_accounts(&batch),
        "a record write is not an account"
    );
    batch.put(b"acct:anything", b"1");
    assert!(touches_accounts(&batch));

    // The batch iterator reports puts and deletes only, so a range delete it
    // cannot see counts as an account write, wherever it points.
    let mut ranged = WriteBatch::default();
    ranged.delete_range(b"acct:".as_slice(), b"acct;".as_slice());
    assert!(touches_accounts(&ranged));
    let mut elsewhere = WriteBatch::default();
    elsewhere.delete_range(b"d:".as_slice(), b"d;".as_slice());
    assert!(touches_accounts(&elsewhere));

    // Through the database: a snapshot-import-shaped batch writing `acct:`.
    let dir = TempDir::new().expect("tempdir");
    let db = sparse_db(&dir);
    let mut rng = ChaCha20Rng::seed_from_u64(5);
    let mut known = Vec::new();
    commit(&db, random_overlay(&mut rng, &mut known), &[1; 32]);
    assert_exact(&db);
    let mut batch = WriteBatch::default();
    let mut key = b"acct:".to_vec();
    key.extend_from_slice(&[0x42; 32]);
    batch.put(&key, account(&mut rng).encode());
    db.write_batch(batch).expect("write");
    assert_eq!(marker(db.raw_db()).unwrap(), None);
    commit(&db, random_overlay(&mut rng, &mut known), &[2; 32]);
    assert_exact(&db);
}

/// Seeds `count` accounts through one commit (the rebuild path).
fn seeded(dir: &TempDir, count: u32) -> StateDB {
    let db = sparse_db(dir);
    let mut overlay = Overlay::new();
    for index in 0..count {
        let address = *blake3::hash(&index.to_le_bytes()).as_bytes();
        overlay.accounts.insert(
            address,
            Account {
                balance: U256::from(index),
                nonce: 0,
            },
        );
    }
    db.commit_journaled(&overlay, &[0xEE; 32], |_| ())
        .expect("seed");
    db
}

fn touch(count: u32, touched: u32) -> Vec<Change> {
    let mut changes: Vec<Change> = (0..touched)
        .map(|index| {
            // Half existing accounts, half new ones.
            let seed = if index % 2 == 0 {
                index * 7 % count
            } else {
                count + index
            };
            let address = *blake3::hash(&seed.to_le_bytes()).as_bytes();
            let account = Account {
                balance: U256::from(u64::from(index) + 1),
                nonce: 1,
            };
            (address, Some(account.encode()))
        })
        .collect();
    changes.sort_unstable_by_key(|change| change.0);
    changes.dedup_by(|a, b| a.0 == b.0);
    changes
}

/// Deletions of `touched` existing accounts: the shape of a revert.
fn untouch(count: u32, touched: u32) -> Vec<Change> {
    let mut changes: Vec<Change> = (0..touched)
        .map(|index| {
            let seed = index * 7 % count;
            (*blake3::hash(&seed.to_le_bytes()).as_bytes(), None)
        })
        .collect();
    changes.sort_unstable_by_key(|change| change.0);
    changes.dedup_by(|a, b| a.0 == b.0);
    changes
}

#[test]
fn work_per_block_follows_touched_keys_not_total_accounts() {
    // Seeks are the cost model: one per branch on a touched path.
    let mut per_size = Vec::new();
    for count in [1_000_u32, 10_000] {
        let dir = TempDir::new().expect("tempdir");
        let db = seeded(&dir, count);
        let reads = |changes: &[Change]| root_after(db.raw_db(), changes).unwrap().unwrap().1;
        let one = reads(&touch(count, 1));
        let hundred = reads(&touch(count, 100));
        let reverted = reads(&untouch(count, 100));
        per_size.push((count, one, hundred, reverted));
    }
    for (count, one, hundred, reverted) in &per_size {
        // About log2(count) branches a key, plus a seek or two at the bottom.
        assert!(*one < 32, "{count} accounts: one key took {one} seeks");
        assert!(
            *hundred < 100 * 32,
            "{count} accounts: 100 keys took {hundred}"
        );
        assert!(
            *reverted < 100 * 32,
            "{count} accounts: 100 deletions took {reverted}"
        );
        // A full rebuild reads every account.
        assert!(*hundred < u64::from(*count));
    }
    // Ten times the accounts adds a few levels, not ten times the work.
    let (_, one_small, _, _) = per_size[0];
    let (_, one_large, _, _) = per_size[1];
    assert!(one_large <= one_small + 8, "{one_small} -> {one_large}");
}

/// Wire bytes of one transaction output: amount (32) and recipient (32), as
/// `core::transaction`'s `OUTPUT_SIZE`.
const OUTPUT_BYTES: u64 = 64;

/// A full block's worth of outputs to `(x, x ^ 1)` pairs at random `x`.
fn neighbour_pairs(rng: &mut ChaCha20Rng) -> Overlay {
    let outputs = maya_fee_market::limits::MAX_BLOCK_BYTES / OUTPUT_BYTES;
    let mut overlay = Overlay::new();
    for _ in 0..outputs / 2 {
        let mut x = [0; 32];
        rng.fill_bytes(&mut x);
        let mut neighbour = x;
        neighbour[31] ^= 1;
        for address in [x, neighbour] {
            overlay.accounts.insert(address, account(rng));
        }
    }
    overlay
}

#[test]
fn neighbour_addresses_cost_one_branch_each_not_a_spine() {
    // The storage attack on a cache that stores every node holding two or
    // more accounts: a pair one bit apart shares ~240 single-child levels
    // nobody else does. Stored that way, a full block of such pairs wrote
    // ~240 entries an account. A tree of N accounts has N - 1 branches, and
    // only branches are stored.
    let dir = TempDir::new().expect("tempdir");
    let db = seeded(&dir, 1_000);
    let mut rng = ChaCha20Rng::seed_from_u64(0xA77AC);
    let mut overlay = neighbour_pairs(&mut rng);
    let created = u64::try_from(overlay.accounts.len()).unwrap();

    let started = Instant::now();
    db.root_with_overlay(&mut overlay).expect("preview");
    let plan = overlay.root_plan.clone().expect("a plan");
    assert!(plan.walked(), "under the floor, the block walks the cache");
    let writes = u64::try_from(plan.delta.len()).unwrap();
    let mut batch = WriteBatch::default();
    super::stage(&mut batch, &plan);
    let bytes = u64::try_from(batch.size_in_bytes()).unwrap();
    let walk_time = started.elapsed();
    db.commit_journaled(&overlay, &[0xA1; 32], |_| ())
        .expect("commit");

    let accounts = u64::try_from(leaves(&db).len()).unwrap();
    let stored = u64::try_from(stored_nodes(&db).len()).unwrap();
    println!(
        "red team: {created} neighbour accounts on 1000: {writes} cache writes, \
         {bytes} batch bytes ({} per account), {stored} stored for {accounts} accounts, \
         walk+stage {walk_time:?}",
        bytes / created
    );
    assert_eq!(
        stored,
        accounts - 1,
        "one branch an account, whatever the addresses"
    );
    assert!(
        writes <= created + 1_000,
        "{writes} writes for {created} new accounts"
    );
    // A put is a 37-byte key and 64-byte value plus batch framing.
    assert!(
        bytes <= created * 128,
        "{bytes} bytes for {created} accounts"
    );
    assert_exact(&db);

    // Touching a pair again reads its path's branches, not its 240 levels.
    let pair = *overlay.accounts.keys().next().unwrap();
    let mut neighbour = pair;
    neighbour[31] ^= 1;
    let changes: Vec<Change> = [pair, neighbour]
        .iter()
        .map(|address| (*address, Some(account(&mut rng).encode())))
        .collect();
    let (_, reads) = root_after(db.raw_db(), &changes).unwrap().unwrap();
    assert!(reads < 64, "a touched pair took {reads} seeks");
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[samples.len() / 2]
}

fn time(runs: usize, mut f: impl FnMut()) -> Duration {
    median(
        (0..runs)
            .map(|_| {
                let started = Instant::now();
                f();
                started.elapsed()
            })
            .collect(),
    )
}

/// Real numbers for the report: `cargo test -p custom-l1-node --lib
/// root_cost_by_state_size -- --ignored --nocapture`. Not a pass/fail on time:
/// a timing assertion on a shared CI runner measures the runner.
#[test]
#[ignore = "measurement; seeds up to 1M accounts"]
fn root_cost_by_state_size() {
    const TOUCHED: u32 = 100;
    const RUNS: usize = 5;
    for count in [1_000_u32, 10_000, 100_000, 1_000_000] {
        let dir = TempDir::new().expect("tempdir");
        let started = Instant::now();
        let db = seeded(&dir, count);
        let seed_time = started.elapsed();
        let changes = touch(count, TOUCHED);
        let mut overlay = Overlay::new();
        for (address, value) in &changes {
            overlay
                .accounts
                .insert(*address, Account::decode(&value.unwrap()).unwrap());
        }

        let walk = time(RUNS, || {
            root_after(db.raw_db(), &changes).unwrap().unwrap();
        });
        // The accounts layer's whole per-block cost: plan, stage and write, as
        // a block pays them once. `plan` directly, not `root_with_overlay`,
        // whose debug-build cross-check is itself a full rebuild.
        let commit = {
            let snapshot_dir = TempDir::new().expect("tempdir");
            db.create_checkpoint(snapshot_dir.path().join("c"))
                .expect("checkpoint");
            let copy = StateDB::open(snapshot_dir.path().join("c")).expect("open copy");
            let started = Instant::now();
            let plan = super::plan(
                copy.raw_db(),
                0,
                changes.clone(),
                &super::Counters::default(),
            )
            .expect("plan");
            let mut batch = WriteBatch::default();
            super::stage(&mut batch, &plan);
            copy.raw_db().write(batch).expect("write");
            let elapsed = started.elapsed();
            assert!(plan.walked());
            elapsed
        };
        let full = time(RUNS, || {
            let mut merged: BTreeMap<Address, Account> =
                db.all_accounts().unwrap().into_iter().collect();
            merged.extend(overlay.accounts.iter().map(|(a, b)| (*a, *b)));
            crate::state::stateless::sparse_accounts_root(&merged).unwrap();
        });
        let verify = time(1, || assert!(db.verify_root_cache().unwrap()));
        let (_, reads) = root_after(db.raw_db(), &changes).unwrap().unwrap();
        println!(
            "accounts={count:>8} touched={TOUCHED}: walk median={walk:?} ({reads} seeks) \
             plan+stage+write={commit:?} full-rebuild median={full:?} \
             verify-at-open={verify:?} seed+cache-build={seed_time:?}"
        );

        if count >= 100_000 {
            // A full block of new accounts: the crossover's case.
            let mut rng = ChaCha20Rng::seed_from_u64(u64::from(count));
            let block = neighbour_pairs(&mut rng);
            let block_changes: Vec<Change> = block
                .accounts
                .iter()
                .map(|(address, account)| (*address, Some(account.encode())))
                .collect();
            let snapshot = db.raw_db().snapshot();
            let (root, accounts) = marker(db.raw_db()).unwrap().unwrap();
            let walked = time(1, || {
                super::walk(&snapshot, &block_changes, root, accounts)
                    .ok()
                    .unwrap();
            });
            let rebuilt = time(1, || {
                super::build_plan(&snapshot, 0, block_changes.clone()).unwrap();
            });
            println!(
                "accounts={count:>8} max block ({} neighbour outputs): walk={walked:?} \
                 rebuild={rebuilt:?} (the plan takes the rebuild: {})",
                block_changes.len(),
                !super::worth_walking(block_changes.len(), accounts)
            );
        }
    }
}
