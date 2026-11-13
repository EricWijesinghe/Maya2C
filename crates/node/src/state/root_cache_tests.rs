//! The root cache against the full rebuild (ADR-062a).
//!
//! The authority is `sparse::root` over every committed account, the function
//! `StateDB::state_root` has always used. Every test here asserts the cache
//! agrees with it: the root, and every stored node.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::time::Instant;

use maya_stateless_core::{Blake3, sparse};
use primitive_types::U256;
use rand_chacha::ChaCha20Rng;
use rand_chacha::rand_core::{RngCore, SeedableRng};
use tempfile::TempDir;

use super::{MARKER, NODES_END, ROOT_CACHE_PREFIX, Walk, marker, node_key, root_after};
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

/// Every stored node, without the marker.
fn stored_nodes(db: &StateDB) -> BTreeMap<Vec<u8>, Vec<u8>> {
    db.scan_prefix(ROOT_CACHE_PREFIX)
        .expect("scan")
        .into_iter()
        .filter(|(key, _)| key.as_slice() != MARKER)
        .collect()
}

/// The nodes a from-scratch build over committed accounts stores.
fn rebuilt_nodes(db: &StateDB) -> BTreeMap<Vec<u8>, Vec<u8>> {
    let mut walk = Walk::new(db.raw_db(), true);
    walk.build(0, [0; 32], &leaves(db));
    walk.delta
        .unwrap()
        .into_iter()
        .map(|(key, digest)| (key, digest.unwrap().to_vec()))
        .collect()
}

/// The cache is valid and equal, node for node, to a rebuild.
fn assert_exact(db: &StateDB) {
    assert_eq!(marker(db.raw_db()).unwrap(), Some(full_root(db)));
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

#[test]
fn the_cache_prefix_is_local_only_and_outside_the_root() {
    assert!(!is_committed(ROOT_CACHE_PREFIX));
    assert!(!is_committed(&node_key(0, &[0; 32])));
    assert!(LOCAL_ONLY_PREFIXES.contains(&ROOT_CACHE_PREFIX));
    // Node keys sit between the marker and the end bound, and only there.
    assert!(MARKER < node_key(0, &[0; 32]).as_slice());
    assert!(node_key(255, &[0xFF; 32]).as_slice() < NODES_END);
}

#[test]
fn incremental_root_equals_full_rebuild_over_random_histories() {
    for seed in 0..12 {
        let dir = TempDir::new().expect("tempdir");
        let db = sparse_db(&dir);
        let mut rng = ChaCha20Rng::seed_from_u64(seed);
        let mut known = Vec::new();
        let mut journal: Vec<[u8; 32]> = Vec::new();

        for step in 0_u64..80 {
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
                    continue;
                }
                _ => {
                    let overlay = random_overlay(&mut rng, &mut known);
                    let predicted = db.root_with_overlay(&overlay).expect("preview");
                    let id = *blake3::hash(&[seed.to_le_bytes(), step.to_le_bytes()].concat())
                        .as_bytes();
                    db.commit_journaled(&overlay, &id, |_| ()).expect("commit");
                    journal.push(id);
                    // The preview, from the cache, is the committed full root.
                    assert_eq!(
                        predicted,
                        db.state_root().expect("root"),
                        "seed {seed} step {step}"
                    );
                }
            }
            assert_exact(&db);
        }
        // Unwinding everything returns to the cache of the starting state.
        while let Some(id) = journal.pop() {
            db.revert_block(&id).expect("revert");
            assert_eq!(marker(db.raw_db()).unwrap(), Some(full_root(&db)));
        }
        assert_exact(&db);
    }
}

#[test]
fn a_dense_post_state_withdraws_the_cache() {
    let dir = TempDir::new().expect("tempdir");
    let db = sparse_db(&dir);
    let mut rng = ChaCha20Rng::seed_from_u64(99);
    let mut known = Vec::new();
    db.commit_journaled(&random_overlay(&mut rng, &mut known), &[1; 32], |_| ())
        .expect("commit");
    assert_exact(&db);

    // A block that removes the `sl:` marker, as a revert past activation does.
    let mut overlay = random_overlay(&mut rng, &mut known);
    overlay.records.insert(SPARSE_ACCOUNTS_KEY.to_vec(), None);
    db.commit_journaled(&overlay, &[2; 32], |_| ())
        .expect("commit");
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
            db.commit_journaled(&random_overlay(&mut rng, &mut known), &[step; 32], |_| ())
                .expect("commit");
        }
        db.state_root().expect("root")
    };

    // A clean restart: the cache is still valid and still exact.
    let db = StateDB::open(dir.path()).expect("reopen");
    assert!(db.verify_root_cache().expect("verify"));
    assert_exact(&db);
    assert_eq!(db.state_root().expect("root"), root);

    // One interior node damaged on disk, with the marker still right: the
    // shallow check would pass it, the node-by-node check does not.
    let (key, _) = stored_nodes(&db).into_iter().next_back().expect("a node");
    db.raw_put_for_test(&key, &[0xAB; 32]).expect("damage");
    drop(db);
    let db = StateDB::open(dir.path()).expect("reopen");
    assert!(!db.verify_root_cache().expect("verify"));
    assert_exact(&db);

    // A stray node no build would write is caught the same way.
    db.raw_put_for_test(&node_key(200, &[0x5A; 32]), &[1; 32])
        .expect("stray");
    assert!(!db.verify_root_cache().expect("verify"));
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

fn touch(count: u32, touched: u32) -> Vec<super::Change> {
    let mut changes: Vec<super::Change> = (0..touched)
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

#[test]
fn work_per_block_follows_touched_keys_not_total_accounts() {
    // Nodes visited is the cost model: one cache read or one seek each.
    let mut per_size = Vec::new();
    for count in [1_000_u32, 10_000] {
        let dir = TempDir::new().expect("tempdir");
        let db = seeded(&dir, count);
        let (_, one) = root_after(db.raw_db(), &touch(count, 1)).unwrap().unwrap();
        let (_, hundred) = root_after(db.raw_db(), &touch(count, 100))
            .unwrap()
            .unwrap();
        per_size.push((count, one, hundred));
    }
    for (count, one, hundred) in &per_size {
        // About two nodes a level, about log2(count) + 2 levels a key.
        assert!(*one < 64, "{count} accounts: one key visited {one}");
        assert!(
            *hundred < 100 * 64,
            "{count} accounts: 100 keys visited {hundred}"
        );
        // A full rebuild touches every account: ~2 nodes each.
        assert!(*hundred < u64::from(*count) * 2);
    }
    // Ten times the accounts adds a few levels, not ten times the work.
    let (_, one_small, _) = per_size[0];
    let (_, one_large, _) = per_size[1];
    assert!(one_large <= one_small + 16, "{one_small} -> {one_large}");
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

        let median = |mut samples: Vec<std::time::Duration>| {
            samples.sort();
            samples[samples.len() / 2]
        };
        let incremental = median(
            (0..RUNS)
                .map(|_| {
                    let started = Instant::now();
                    root_after(db.raw_db(), &changes).unwrap().unwrap();
                    started.elapsed()
                })
                .collect(),
        );
        let full = median(
            (0..RUNS)
                .map(|_| {
                    let started = Instant::now();
                    let mut merged: BTreeMap<Address, Account> =
                        db.all_accounts().unwrap().into_iter().collect();
                    merged.extend(overlay.accounts.iter().map(|(a, b)| (*a, *b)));
                    crate::state::stateless::sparse_accounts_root(&merged).unwrap();
                    started.elapsed()
                })
                .collect(),
        );
        let (cached, visited) = root_after(db.raw_db(), &changes).unwrap().unwrap();
        let mut merged: BTreeMap<Address, Account> =
            db.all_accounts().unwrap().into_iter().collect();
        merged.extend(overlay.accounts.iter().map(|(a, b)| (*a, *b)));
        assert_eq!(
            cached,
            crate::state::stateless::sparse_accounts_root(&merged).unwrap()
        );
        println!(
            "accounts={count:>8} touched={TOUCHED}: incremental median={incremental:?} \
             (visited {visited} nodes) full-rebuild median={full:?} seed+cache-build={seed_time:?}"
        );
    }
}
