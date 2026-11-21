//! CSTATE-1 and CSTATE-2 (ADR-142 amendment 6) against their references.
//!
//! The reference for the running total is a scan of every committed entry;
//! the reference for a block's total is the merged map the contracts layer
//! folds. Every step of a randomized history of deployments, writes,
//! overwrites, refusals, reverts and reorgs asserts both agree, and that the
//! root a block previews is the root it commits and the root a revert
//! restores.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::sync::Arc;

use maya_governance::params::ParameterKey;
use rand_chacha::ChaCha20Rng;
use rand_chacha::rand_core::{RngCore, SeedableRng};
use tempfile::TempDir;

use super::{ENTRY_OVERHEAD_BYTES, admit_block_writes, admit_state_size, entry_size};
use crate::error::NodeError;
use crate::governance::record::ParameterTable;
use crate::state::db::{Overlay, StateDB};

fn open() -> (StateDB, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let db = StateDB::open(dir.path()).expect("open");
    (db, dir)
}

/// Sets the two ceilings straight in committed state.
fn set_caps(db: &StateDB, writes: u64, bytes: u64) {
    let table = ParameterTable::new()
        .with(
            ParameterKey::VmMaxBlockContractWrites,
            maya_dex::Amount::from_u64(writes),
        )
        .with(
            ParameterKey::VmMaxContractStateBytes,
            maya_dex::Amount::from_u64(bytes),
        );
    db.raw_put_for_test(crate::governance::PARAMETERS_KEY, &table.encode())
        .expect("caps");
}

/// What CSTATE-2 counts for the state `overlay` would commit, from the map
/// the contracts layer folds.
fn reference_total(db: &StateDB, overlay: &Overlay) -> u64 {
    db.merged_contracts(overlay)
        .expect("merged")
        .iter()
        .map(|(key, value)| entry_size(key.len(), value.len()))
        .sum()
}

/// One storage write through the admission a call uses, staged on success.
fn write(
    db: &StateDB,
    overlay: &mut Overlay,
    contract: [u8; 32],
    key: Vec<u8>,
    value: Vec<u8>,
) -> crate::error::Result<()> {
    let writes = BTreeMap::from([(key, value)]);
    db.admit_contract_storage(overlay, &contract, &writes)?;
    let staged = Arc::make_mut(&mut overlay.contract_storage);
    for (key, value) in writes {
        staged.insert((contract, key), value);
    }
    Ok(())
}

/// One deployment through the admission a deployment uses.
fn deploy(
    db: &StateDB,
    overlay: &mut Overlay,
    contract: [u8; 32],
    code: Vec<u8>,
) -> crate::error::Result<()> {
    db.admit_contract_code(overlay, &contract, &code)?;
    overlay.code.insert(contract, code);
    Ok(())
}

#[test]
fn an_entry_counts_its_key_its_value_and_the_overhead() {
    assert_eq!(entry_size(0, 0), ENTRY_OVERHEAD_BYTES);
    assert_eq!(entry_size(43, 1), 44 + ENTRY_OVERHEAD_BYTES);
    assert_eq!(entry_size(usize::MAX, usize::MAX), u64::MAX);
}

#[test]
fn only_growth_past_a_cap_is_refused() {
    assert!(admit_block_writes(0, 10, 10).is_ok());
    assert_eq!(
        admit_block_writes(10, 11, 10),
        Err(NodeError::BlockContractWritesExhausted {
            entries: 11,
            cap: 10
        })
    );
    // Already past a lowered cap: a write that adds no entry still applies.
    assert!(admit_block_writes(20, 20, 10).is_ok());
    assert!(admit_state_size(0, 100, 100).is_ok());
    assert_eq!(
        admit_state_size(100, 101, 100),
        Err(NodeError::ContractStateFull {
            size: 101,
            cap: 100
        })
    );
    // An overwrite that does not grow the total, or shrinks it, past the cap.
    assert!(admit_state_size(500, 500, 100).is_ok());
    assert!(admit_state_size(500, 400, 100).is_ok());
}

#[test]
fn a_block_over_the_write_cap_is_refused_and_stages_nothing() {
    let (db, _dir) = open();
    set_caps(&db, 2, u64::MAX);
    let mut overlay = Overlay::new();
    write(&db, &mut overlay, [1; 32], b"a".to_vec(), vec![1]).unwrap();
    write(&db, &mut overlay, [1; 32], b"b".to_vec(), vec![1]).unwrap();
    let before = overlay.contract_growth;
    assert_eq!(
        write(&db, &mut overlay, [1; 32], b"c".to_vec(), vec![1]),
        Err(NodeError::BlockContractWritesExhausted { entries: 3, cap: 2 })
    );
    assert_eq!(overlay.contract_growth, before);
    assert_eq!(overlay.contract_storage.len(), 2);
    // A key the block already wrote is not a new entry.
    write(&db, &mut overlay, [1; 32], b"a".to_vec(), vec![2; 9]).unwrap();
    // Revealed sealed actions have a quarter more (here 2 / 4 = 0).
    overlay.revealing = true;
    assert!(write(&db, &mut overlay, [1; 32], b"c".to_vec(), vec![1]).is_err());
}

#[test]
fn contract_state_past_the_ceiling_is_refused_but_never_stranded() {
    let (db, _dir) = open();
    let slot = entry_size(7 + 32 + 1, 8);
    set_caps(&db, 1_024, 2 * slot);
    let mut overlay = Overlay::new();
    write(&db, &mut overlay, [1; 32], b"a".to_vec(), vec![1; 8]).unwrap();
    write(&db, &mut overlay, [1; 32], b"b".to_vec(), vec![1; 8]).unwrap();
    db.commit_journaled(&overlay, &[1; 32], |_| ()).unwrap();
    assert_eq!(db.contract_state_size().unwrap(), 2 * slot);

    let mut overlay = Overlay::new();
    assert_eq!(
        write(&db, &mut overlay, [1; 32], b"c".to_vec(), Vec::new()),
        Err(NodeError::ContractStateFull {
            size: 2 * slot + slot - 8,
            cap: 2 * slot
        })
    );
    assert!(matches!(
        deploy(&db, &mut overlay, [2; 32], vec![0; 4]),
        Err(NodeError::ContractStateFull { .. })
    ));
    // Overwriting at the same size, then smaller, then back up to the cap.
    write(&db, &mut overlay, [1; 32], b"a".to_vec(), vec![2; 8]).unwrap();
    write(&db, &mut overlay, [1; 32], b"a".to_vec(), vec![2; 1]).unwrap();
    write(&db, &mut overlay, [1; 32], b"a".to_vec(), vec![3; 8]).unwrap();
    assert!(write(&db, &mut overlay, [1; 32], b"a".to_vec(), vec![3; 9]).is_err());

    // A ceiling lowered below the state: overwrites still apply, growth not.
    set_caps(&db, 1_024, slot);
    let mut overlay = Overlay::new();
    write(&db, &mut overlay, [1; 32], b"b".to_vec(), vec![4; 2]).unwrap();
    write(&db, &mut overlay, [1; 32], b"b".to_vec(), vec![4; 1]).unwrap();
    assert!(write(&db, &mut overlay, [1; 32], b"b".to_vec(), vec![4; 3]).is_err());
    assert!(write(&db, &mut overlay, [1; 32], b"d".to_vec(), Vec::new()).is_err());
}

#[test]
fn the_running_total_follows_writes_that_bypass_a_commit() {
    let (db, _dir) = open();
    let mut overlay = Overlay::new();
    write(&db, &mut overlay, [1; 32], b"a".to_vec(), vec![1; 8]).unwrap();
    db.commit_journaled(&overlay, &[1; 32], |_| ()).unwrap();
    assert_eq!(
        db.contract_state_size().unwrap(),
        db.contract_state_size_scan().unwrap()
    );
    // A raw write moves the generation, so the next reader counts again.
    let mut key = crate::state::db::contract_storage_key(&[5; 32], b"x");
    db.raw_put_for_test(&key, &[0; 100]).unwrap();
    assert_eq!(
        db.contract_state_size().unwrap(),
        db.contract_state_size_scan().unwrap()
    );
    key.push(1);
    db.raw_put_for_test(&key, &[0; 7]).unwrap();
    db.revert_block(&[1; 32]).unwrap();
    assert_eq!(
        db.contract_state_size().unwrap(),
        db.contract_state_size_scan().unwrap()
    );
}

/// One random block on top of `db`: deployments and writes over a small key
/// space (so overwrites of committed and staged keys both happen), some of
/// them refused by caps chosen near the state's size.
fn random_block(
    db: &StateDB,
    rng: &mut ChaCha20Rng,
    contracts: &mut Vec<[u8; 32]>,
    refused: &mut u32,
) -> Overlay {
    let mut overlay = Overlay::new();
    let ops = 1 + rng.next_u32() % 12;
    for _ in 0..ops {
        let before = overlay.contract_growth;
        let entries_before = overlay.code.len() + overlay.contract_storage.len();
        let result = if contracts.is_empty() || rng.next_u32().is_multiple_of(5) {
            let mut id = [0u8; 32];
            rng.fill_bytes(&mut id);
            let mut code = vec![0u8; (rng.next_u32() % 300) as usize];
            rng.fill_bytes(&mut code);
            let deployed = deploy(db, &mut overlay, id, code);
            if deployed.is_ok() {
                contracts.push(id);
            }
            deployed
        } else {
            let contract = contracts[(rng.next_u32() as usize) % contracts.len()];
            let key = vec![(rng.next_u32() % 6) as u8];
            let mut value = vec![0u8; (rng.next_u32() % 40) as usize];
            rng.fill_bytes(&mut value);
            write(db, &mut overlay, contract, key, value)
        };
        match result {
            Ok(()) => {}
            Err(
                NodeError::ContractStateFull { .. }
                | NodeError::BlockContractWritesExhausted { .. },
            ) => {
                *refused += 1;
                // A refusal leaves the block exactly as it was.
                assert_eq!(overlay.contract_growth, before);
                assert_eq!(
                    overlay.code.len() + overlay.contract_storage.len(),
                    entries_before
                );
            }
            Err(other) => panic!("unexpected {other}"),
        }
        // The block's running total is the size of the map the layer folds.
        let base = db.contract_state_size_scan().unwrap();
        assert_eq!(
            overlay.contract_growth.total(base),
            reference_total(db, &overlay)
        );
    }
    overlay
}

fn forget_roots(applied: &mut [([u8; 32], Option<[u8; 32]>)]) {
    for entry in applied {
        entry.1 = None;
    }
}

#[test]
fn the_running_total_and_the_root_agree_with_the_full_fold_over_random_histories() {
    let (mut refused, mut reverts_checked) = (0u32, 0u32);
    for seed in 0..12u64 {
        let mut rng = ChaCha20Rng::seed_from_u64(0xC57A_7E00 + seed);
        let (db, _dir) = open();
        let mut contracts = Vec::new();
        // (block id, root before it), newest last: what a revert must restore.
        // `None` once a raw write landed after it, which no revert undoes.
        let mut applied: Vec<([u8; 32], Option<[u8; 32]>)> = Vec::new();
        let mut next_id = 0u64;
        for _ in 0..40 {
            // Caps near the current state, so refusals happen on most seeds.
            let size = db.contract_state_size_scan().unwrap();
            if rng.next_u32() % 3 == 0 {
                let headroom = u64::from(rng.next_u32() % 2_000);
                set_caps(&db, u64::from(2 + rng.next_u32() % 10), size + headroom);
                forget_roots(&mut applied);
            }
            match rng.next_u32() % 6 {
                // Revert one block, or a reorg of up to three.
                0 if !applied.is_empty() => {
                    let depth = 1 + (rng.next_u32() as usize) % applied.len().min(3);
                    for _ in 0..depth {
                        let (id, root_before) = applied.pop().unwrap();
                        db.revert_block(&id).unwrap();
                        if let Some(root_before) = root_before {
                            assert_eq!(db.state_root().unwrap(), root_before, "seed {seed}");
                            reverts_checked += 1;
                        }
                    }
                    // A reverted deployment's id may come back; forget them
                    // all so the next block deploys fresh ids.
                    contracts.retain(|id| db.get_code(id).unwrap().is_some());
                }
                // A write outside any commit (snapshot import, a repair).
                1 => {
                    let key = crate::state::db::contract_storage_key(
                        &[0xEE; 32],
                        &[seed.to_le_bytes()[0]],
                    );
                    db.raw_put_for_test(&key, &vec![7; (rng.next_u32() % 50) as usize])
                        .unwrap();
                    forget_roots(&mut applied);
                }
                _ => {
                    let root_before = db.state_root().unwrap();
                    let mut overlay = random_block(&db, &mut rng, &mut contracts, &mut refused);
                    let previewed = db.root_with_overlay(&mut overlay).unwrap();
                    next_id += 1;
                    let mut id = [0u8; 32];
                    id[..8].copy_from_slice(&next_id.to_le_bytes());
                    id[8..16].copy_from_slice(&seed.to_le_bytes());
                    db.commit_journaled(&overlay, &id, |_| ()).unwrap();
                    assert_eq!(db.state_root().unwrap(), previewed, "seed {seed}");
                    applied.push((id, Some(root_before)));
                }
            }
            assert_eq!(
                db.contract_state_size().unwrap(),
                db.contract_state_size_scan().unwrap(),
                "seed {seed}"
            );
            assert_eq!(
                db.contract_state_size().unwrap(),
                reference_total(&db, &Overlay::new()),
                "seed {seed}"
            );
        }
    }
    eprintln!(
        "random histories: {refused} refusals, {reverts_checked} reverts checked against the root"
    );
    // The history must exercise what it claims to.
    assert!(
        refused > 0 && reverts_checked > 0,
        "{refused} refused, {reverts_checked} reverts checked"
    );
}
