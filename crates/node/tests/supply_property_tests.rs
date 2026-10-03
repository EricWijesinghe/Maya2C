//! Randomised transaction sequences never change total supply
//! (Master Prompt 3 §2: "random transaction sequences never change total
//! supply except through defined mint/burn rules").
//!
//! A seeded generator rather than proptest: every signature here is a real
//! hybrid ML-DSA-65 + SLH-DSA signature (~0.1 s each), so the case count is
//! chosen for runtime, and a fixed seed makes a failure a regression test by
//! itself. Blocks mix valid transfers, overdrafts, replayed and skipped nonces
//! and self-transfers; a block with any invalid transaction must be refused
//! whole and leave the state root untouched.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation,
    clippy::single_match_else
)]

use custom_l1_node::core::{Block, BlockHeader, Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};
use tempfile::TempDir;

mod common;

const HOLDERS: usize = 4;
const BLOCKS: usize = 30;

fn next(x: &mut u64) -> u64 {
    *x ^= *x << 13;
    *x ^= *x >> 7;
    *x ^= *x << 17;
    *x
}

fn block_of(transactions: Vec<Transaction>) -> Block {
    Block::new(
        BlockHeader {
            prev_hash: [0u8; 32],
            state_root: [0u8; 32],
            timestamp: 1_756_252_800,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        transactions,
    )
}

fn supply(db: &StateDB, addrs: &[Address]) -> u128 {
    addrs
        .iter()
        .map(|a| u128::from(db.get_account(a).unwrap().balance))
        .sum()
}

#[test]
fn random_transfer_sequences_conserve_supply_and_failed_blocks_change_nothing() {
    let dir = TempDir::new().unwrap();
    let db = StateDB::open(dir.path()).unwrap();
    common::bind(&db);
    let keys: Vec<HybridSigningKey> = (0..HOLDERS)
        .map(|_| generate_signing_key().unwrap())
        .collect();
    let mut addrs: Vec<Address> = keys.iter().map(HybridSigningKey::address).collect();
    addrs.push([0xEE; 32]); // a receive-only account
    for k in &keys {
        db.put_account(
            &k.address(),
            &Account {
                balance: 10_000,
                nonce: 0,
            },
        )
        .unwrap();
    }
    let total = supply(&db, &addrs);
    let mut nonces = [0u64; HOLDERS];
    let mut seed = 0x5EED_u64;
    let (mut applied, mut refused) = (0, 0);
    for _ in 0..BLOCKS {
        let count = 1 + (next(&mut seed) % 3) as usize;
        let mut txs = Vec::new();
        let mut local = nonces;
        for _ in 0..count {
            let from = (next(&mut seed) % HOLDERS as u64) as usize;
            let to = addrs[(next(&mut seed) % addrs.len() as u64) as usize];
            // Mostly affordable, sometimes an overdraft.
            let amount = if next(&mut seed).is_multiple_of(5) {
                50_000
            } else {
                next(&mut seed) % 3_000
            };
            // Mostly the right nonce, sometimes a replay or a gap.
            let nonce = match next(&mut seed) % 8 {
                0 => local[from].wrapping_sub(1),
                1 => local[from] + 1,
                _ => local[from],
            };
            let mut tx = Transaction::new(
                vec![],
                vec![TxOutput {
                    amount,
                    recipient: to,
                }],
                nonce,
            );
            tx.sign(&keys[from], &common::test_chain()).unwrap();
            txs.push(tx);
            local[from] = local[from].max(nonce.wrapping_add(1));
        }
        let before = db.state_root().unwrap();
        match db.apply_block(&block_of(txs.clone()), BlockContext::GENESIS) {
            Ok(_) => {
                applied += 1;
                for tx in &txs {
                    let from = keys
                        .iter()
                        .position(|k| k.address() == tx.sender())
                        .unwrap();
                    nonces[from] += 1;
                }
            }
            Err(_) => {
                refused += 1;
                assert_eq!(
                    db.state_root().unwrap(),
                    before,
                    "a refused block changed state"
                );
            }
        }
        assert_eq!(supply(&db, &addrs), total, "supply changed");
    }
    println!(
        "{BLOCKS} random blocks: {applied} applied, {refused} refused whole; supply constant at {total}"
    );
    assert!(
        applied > 0 && refused > 0,
        "the generator must exercise both paths"
    );
}
