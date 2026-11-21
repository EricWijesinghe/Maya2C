//! The contracts root cache against the full fold (ADR-142 amendment 5).
//!
//! The authority is `StateDB::contracts_root_full`, the fold every root used
//! before the cache existed. Every test here asserts the cache agrees with it,
//! and that it got there by following commits rather than by re-reading
//! everything (which would agree trivially and save nothing).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Instant;

use rand_chacha::ChaCha20Rng;
use rand_chacha::rand_core::{RngCore, SeedableRng};
use rocksdb::WriteBatch;
use tempfile::TempDir;

use super::{UNCHECKED, changes};
use crate::error::NodeError;
use crate::state::db::{Overlay, RootSource, StateDB, code_key, contract_storage_key};
use crate::state::preview::{PreviewCache, landed};

type Contract = [u8; 32];
type Digest = [u8; 32];

/// Contracts and slots the history has written, so later steps overwrite them.
#[derive(Default)]
struct Known {
    contracts: Vec<Contract>,
    slots: Vec<(Contract, Vec<u8>)>,
}

fn pick(rng: &mut ChaCha20Rng, below: usize) -> usize {
    usize::try_from(rng.next_u64() % u64::try_from(below).unwrap()).unwrap()
}

fn bytes(rng: &mut ChaCha20Rng, max: usize) -> Vec<u8> {
    let mut out = vec![0; pick(rng, max + 1)];
    rng.fill_bytes(&mut out);
    out
}

/// A contract id that often shares a long prefix with one already known, so
/// new entries land at the front, the middle and the end of the leaf order.
fn contract(rng: &mut ChaCha20Rng, known: &Known) -> Contract {
    let mut id = [0; 32];
    rng.fill_bytes(&mut id);
    if !known.contracts.is_empty() && rng.next_u64().is_multiple_of(2) {
        let near = known.contracts[pick(rng, known.contracts.len())];
        id[..31].copy_from_slice(&near[..31]);
    }
    id
}

/// A block's contract writes: deployments, new slots (keys that are prefixes
/// of each other included), overwrites, empty values; sometimes none at all.
fn random_overlay(rng: &mut ChaCha20Rng, known: &mut Known) -> Overlay {
    let mut overlay = Overlay::new();
    let mut address = [0; 32];
    rng.fill_bytes(&mut address);
    overlay
        .accounts
        .insert(address, crate::state::account::Account::default());
    if rng.next_u64().is_multiple_of(5) {
        return overlay;
    }
    for _ in 0..=pick(rng, 12) {
        match rng.next_u64() % 4 {
            0 => {
                let id = contract(rng, known);
                overlay.code.insert(id, bytes(rng, 48));
                known.contracts.push(id);
            }
            1 if !known.slots.is_empty() => {
                let slot = known.slots[pick(rng, known.slots.len())].clone();
                std::sync::Arc::make_mut(&mut overlay.contract_storage)
                    .insert(slot, bytes(rng, 64));
            }
            _ => {
                let id = if known.contracts.is_empty() {
                    contract(rng, known)
                } else {
                    known.contracts[pick(rng, known.contracts.len())]
                };
                let mut key = bytes(rng, 3);
                if !known.slots.is_empty() && rng.next_u64().is_multiple_of(3) {
                    key = known.slots[pick(rng, known.slots.len())].1.clone();
                    key.push(u8::try_from(pick(rng, 256)).unwrap());
                }
                std::sync::Arc::make_mut(&mut overlay.contract_storage)
                    .insert((id, key.clone()), bytes(rng, 64));
                known.slots.push((id, key));
            }
        }
    }
    overlay
}

/// The cache and the full fold agree on `overlay` over committed state, for
/// the layer and for the whole state root.
fn assert_agree(db: &StateDB, overlay: &Overlay) {
    assert_eq!(
        db.contracts_root_cached(overlay).unwrap(),
        db.contracts_root_full(overlay).unwrap()
    );
    let (full, _) = db.root_from(overlay, RootSource::Full).unwrap();
    let (cached, _) = db.root_from(overlay, RootSource::Cache).unwrap();
    assert_eq!(cached, full);
}

/// Previews then commits `overlay` under `id`, as a block does.
fn commit(db: &StateDB, mut overlay: Overlay, id: &Digest) {
    assert_agree(db, &overlay);
    db.root_with_overlay(&mut overlay).unwrap();
    db.commit_journaled(&overlay, id, |_| ()).unwrap();
}

fn block_id(counter: &mut u64) -> Digest {
    *counter += 1;
    let mut id = [0; 32];
    id[..8].copy_from_slice(&counter.to_le_bytes());
    id
}

#[test]
fn the_cache_matches_the_full_fold_over_random_histories() {
    // Deploy, write, overwrite, revert (the only deletion: a reverted block's
    // new entries go), reorg, restart and an out-of-band write, from one seed.
    let dir = TempDir::new().unwrap();
    let mut db = StateDB::open(dir.path()).unwrap();
    let mut rng = ChaCha20Rng::seed_from_u64(0x0C0D_E142);
    let mut known = Known::default();
    let mut applied: Vec<Digest> = Vec::new();
    let mut counter = 0;
    // Rebuilds this database instance should have made: one, plus one after
    // every write the cache cannot follow. Anything more means a commit or a
    // revert was not followed.
    let mut expected_rebuilds = 1;
    let (mut commits, mut reverts, mut restarts) = (0_u64, 0_u64, 0_u32);
    // Commits and reverts the cache followed, over every instance.
    let mut followed = 0;
    for _ in 0..400 {
        match rng.next_u64() % 12 {
            0..=6 => {
                let id = block_id(&mut counter);
                commit(&db, random_overlay(&mut rng, &mut known), &id);
                applied.push(id);
                commits += 1;
            }
            7 | 8 => {
                // A reorg: back off up to three blocks, then a new branch.
                for _ in 0..=pick(&mut rng, 3) {
                    if let Some(id) = applied.pop() {
                        db.revert_block(&id).unwrap();
                        reverts += 1;
                    }
                }
                if rng.next_u64().is_multiple_of(2) {
                    let id = block_id(&mut counter);
                    commit(&db, random_overlay(&mut rng, &mut known), &id);
                    applied.push(id);
                    commits += 1;
                }
            }
            9 => {
                followed += db.contracts_root_stats().followed;
                drop(db);
                db = StateDB::open(dir.path()).unwrap();
                expected_rebuilds = 1;
                restarts += 1;
            }
            _ => {
                // A write outside every commit path: the cache goes stale.
                let id = contract(&mut rng, &known);
                db.raw_put_for_test(&contract_storage_key(&id, b"oob"), &bytes(&mut rng, 8))
                    .unwrap();
                expected_rebuilds += 1;
            }
        }
        assert_agree(&db, &Overlay::new());
        assert_eq!(db.contracts_root_stats().rebuilds, expected_rebuilds);
    }
    assert!(
        commits > 100 && reverts > 30 && restarts > 10,
        "{commits} {reverts} {restarts}"
    );
    // Every commit and every revert was followed, none rebuilt from.
    followed += db.contracts_root_stats().followed;
    assert_eq!(followed, commits + reverts);
    assert_eq!(db.contracts_root_stats().repairs, 0);
    assert_eq!(
        db.contracts_root_cached(&Overlay::new())
            .unwrap()
            .map(|r| r != [0; 32]),
        Some(true)
    );
}

#[test]
fn a_root_with_nothing_staged_reuses_the_kept_root() {
    let dir = TempDir::new().unwrap();
    let db = StateDB::open(dir.path()).unwrap();
    let mut overlay = Overlay::new();
    overlay.code.insert([7; 32], b"code".to_vec());
    commit(&db, overlay, &[1; 32]);
    let first = db.contracts_root_cached(&Overlay::new()).unwrap();
    let reused = db.contracts_root_stats().reused;
    assert_eq!(db.contracts_root_cached(&Overlay::new()).unwrap(), first);
    assert_eq!(db.contracts_root_stats().reused, reused + 1);
    assert_eq!(first, db.contracts_root_full(&Overlay::new()).unwrap());
}

#[test]
fn no_contract_means_no_layer_from_either_source() {
    let dir = TempDir::new().unwrap();
    let db = StateDB::open(dir.path()).unwrap();
    assert_eq!(db.contracts_root_cached(&Overlay::new()).unwrap(), None);
    assert_eq!(db.contracts_root_full(&Overlay::new()).unwrap(), None);
}

#[test]
fn a_write_leaves_the_generation_landed_names() {
    let cache = PreviewCache::default();
    let (guard, from) = cache.begin_write_from();
    drop(guard);
    assert_eq!(cache.generation(), landed(from));
}

#[test]
fn a_batch_with_an_unreported_operation_cannot_be_followed() {
    let mut followed = WriteBatch::default();
    followed.put(code_key(&[1; 32]), b"code");
    followed.delete(contract_storage_key(&[1; 32], b"k"));
    followed.put(b"acct:x", b"not a contract entry");
    let seen = changes(&followed).unwrap();
    assert_eq!(seen.len(), 2);
    assert!(seen[0].1.is_some() && seen[1].1.is_none());

    let mut ranged = WriteBatch::default();
    ranged.put(code_key(&[1; 32]), b"code");
    ranged.delete_range(b"cstate:".as_slice(), b"cstate;".as_slice());
    assert!(changes(&ranged).is_none());
}

/// A database whose cache holds two contracts' entries, then damaged.
fn damaged() -> (StateDB, TempDir, Overlay) {
    let dir = TempDir::new().unwrap();
    let db = StateDB::open(dir.path()).unwrap();
    let mut rng = ChaCha20Rng::seed_from_u64(7);
    let mut known = Known::default();
    for step in 0..4_u8 {
        let mut overlay = random_overlay(&mut rng, &mut known);
        overlay.code.insert([step; 32], vec![step]);
        commit(&db, overlay, &[step; 32]);
    }
    db.contracts_root_cached(&Overlay::new()).unwrap();
    db.contracts_root.damage_for_test();
    let mut next = Overlay::new();
    next.code.insert([0xEE; 32], b"next".to_vec());
    (db, dir, next)
}

#[test]
#[should_panic(expected = "contracts root cache disagrees")]
fn the_debug_cross_check_fails_a_unit_test_on_a_wrong_cache() {
    let (db, _dir, next) = damaged();
    let _ = db.root_from(&next, RootSource::Cache);
}

#[test]
fn a_wrong_cache_is_refuted_by_a_block_and_withdrawn() {
    let (db, _dir, next) = damaged();
    UNCHECKED.with(|unchecked| unchecked.set(true));
    let (full, _) = db.root_from(&next, RootSource::Full).unwrap();
    let (wrong, _) = db.root_from(&next, RootSource::Cache).unwrap();
    assert_ne!(wrong, full, "the damage reaches the root unchecked");

    // A block declaring another root still fails, and blames nothing.
    let mut overlay = next.clone();
    let refused = db.check_declared_root(&mut overlay, wrong, &[0xAB; 32]);
    assert!(matches!(refused, Err(NodeError::StateRootMismatch { .. })));
    assert_eq!(db.contracts_root_stats().repairs, 0);

    // A block declaring the full fold's root is accepted on it.
    let accepted = db.check_declared_root(&mut overlay, wrong, &full).unwrap();
    assert_eq!(accepted, full);
    assert_eq!(db.contracts_root_stats().repairs, 1);
    let (again, _) = db.root_from(&next, RootSource::Cache).unwrap();
    assert_eq!(again, full, "rebuilt from the database");
    UNCHECKED.with(|unchecked| unchecked.set(false));
}

#[test]
fn a_confirm_repairs_a_wrong_cache_before_a_header_carries_it() {
    let (db, _dir, next) = damaged();
    UNCHECKED.with(|unchecked| unchecked.set(true));
    let (full, _) = db.root_from(&next, RootSource::Full).unwrap();
    let (confirmed, _) = db.root_from(&next, RootSource::Confirmed).unwrap();
    assert_eq!(confirmed, full);
    assert_eq!(db.contracts_root_stats().repairs, 1);
    let (cached, _) = db.root_from(&next, RootSource::Cache).unwrap();
    assert_eq!(cached, full);
    UNCHECKED.with(|unchecked| unchecked.set(false));
}

/// Median of five runs of `f`, in milliseconds.
fn median_ms(mut f: impl FnMut()) -> f64 {
    let mut runs: Vec<f64> = (0..5)
        .map(|_| {
            let started = Instant::now();
            f();
            started.elapsed().as_secs_f64() * 1e3
        })
        .collect();
    runs.sort_by(f64::total_cmp);
    runs[2]
}

/// `count` contract writes of `len`-byte values to fresh slots of one contract.
fn fresh_writes(tag: u8, count: u32, len: usize) -> Overlay {
    let mut overlay = Overlay::new();
    let staged = std::sync::Arc::make_mut(&mut overlay.contract_storage);
    for i in 0..count {
        let mut key = vec![tag];
        key.extend_from_slice(&i.to_be_bytes());
        staged.insert(([9; 32], key), vec![0xA5; len]);
    }
    overlay
}

#[test]
#[ignore = "timing: run with --ignored --nocapture; figures go in ADR-142 amendment 5"]
fn the_contracts_root_at_400_000_entries_before_and_after() {
    // Before: the full fold, what every root ran until this cache. After:
    // the cache, cold (one scan) and warm. 245 writes of 4 KiB is what one
    // 250 M-gas call can stage (PR #549 review R-1). The last line is a whole
    // block's state-root work on the warm path: its preview, then its commit
    // following the cache.
    const ENTRIES: u32 = 400_000;
    let dir = TempDir::new().unwrap();
    let db = StateDB::open(dir.path()).unwrap();
    for chunk in 0..ENTRIES / 10_000 {
        let mut batch = WriteBatch::default();
        for i in chunk * 10_000..(chunk + 1) * 10_000 {
            batch.put(contract_storage_key(&[9; 32], &i.to_be_bytes()), [1]);
        }
        db.write_batch(batch).unwrap();
    }
    let one = fresh_writes(0x01, 1, 32);
    let heavy = fresh_writes(0x02, 245, 4_096);
    let before = median_ms(|| {
        db.contracts_root_full(&one).unwrap();
    });
    let cold = median_ms(|| {
        db.contracts_root.withdraw("timing: cold start");
        db.contracts_root_cached(&one).unwrap();
    });
    let nothing = median_ms(|| {
        db.contracts_root_cached(&Overlay::new()).unwrap();
    });
    let warm_one = median_ms(|| {
        db.contracts_root_cached(&one).unwrap();
    });
    let warm_heavy = median_ms(|| {
        db.contracts_root_cached(&heavy).unwrap();
    });
    let before_heavy = median_ms(|| {
        db.contracts_root_full(&heavy).unwrap();
    });
    assert_eq!(
        db.contracts_root_cached(&heavy).unwrap(),
        db.contracts_root_full(&heavy).unwrap()
    );
    let mut height = 0_u8;
    let block = median_ms(|| {
        height += 1;
        let overlay = fresh_writes(0x10 + height, 245, 4_096);
        db.contracts_root_cached(&overlay).unwrap();
        db.commit_journaled(&overlay, &[height; 32], |_| ())
            .unwrap();
    });
    eprintln!("TIMING contracts root, {ENTRIES} committed 1 B entries, median of 5:");
    eprintln!("TIMING   before (full fold), 1 new entry        {before:>9.1} ms");
    eprintln!("TIMING   before (full fold), 245 x 4 KiB new    {before_heavy:>9.1} ms");
    eprintln!("TIMING   after, cold (one scan), 1 new entry    {cold:>9.1} ms");
    eprintln!("TIMING   after, warm, nothing staged            {nothing:>9.3} ms");
    eprintln!("TIMING   after, warm, 1 new entry               {warm_one:>9.1} ms");
    eprintln!("TIMING   after, warm, 245 x 4 KiB new           {warm_heavy:>9.1} ms");
    eprintln!("TIMING   after, warm, 245 x 4 KiB root + commit {block:>9.1} ms");
}
