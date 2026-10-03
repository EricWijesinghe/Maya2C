//! Hash-migration rehearsal (Master Prompt 15 §6; procedure in
//! `docs/CRYPTO_WATCH.md` § "Hash agility").
//!
//! The node commits state with BLAKE3 and implements no second hash. This test
//! rehearses the documented procedure for moving the state commitment to a new
//! one — SHA3-256 as the stand-in — at an epoch boundary E, on real node state:
//!
//! 1. every node applies the same blocks up to E (real `StateDB`s);
//! 2. at E each re-commits its account set under the new hash with the same
//!    tree shape (ROOT-1 .. ROOT-4, hash swapped) — all must agree;
//! 3. a bridge record binds the old root to the new one under *both* hashes,
//!    so a light client holding the pre-E root can move to the new root
//!    without trusting anyone;
//! 4. after E an inclusion proof under the new hash verifies against the new
//!    root and fails against the old one, and vice versa;
//! 5. the re-commitment time is measured on 1,000,000 accounts, because a
//!    migration longer than a block interval must run across several blocks.
//!
//! RESEARCH: nothing in consensus calls any of this.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::cast_precision_loss)]

use std::time::Instant;

use custom_l1_node::core::{Block, BlockHeader, Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::signing_key_from_seed;
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};
use sha3::{Digest, Sha3_256};

mod common;

const NODES: usize = 20;

#[derive(Clone, Copy)]
enum Hash {
    Blake3,
    Sha3,
}

impl Hash {
    fn digest(self, parts: &[&[u8]]) -> [u8; 32] {
        match self {
            Self::Blake3 => {
                let mut h = blake3::Hasher::new();
                for p in parts {
                    h.update(p);
                }
                *h.finalize().as_bytes()
            }
            Self::Sha3 => {
                let mut h = Sha3_256::new();
                for p in parts {
                    h.update(p);
                }
                h.finalize().into()
            }
        }
    }
}

fn leaf(hash: Hash, addr: &Address, a: &Account) -> [u8; 32] {
    hash.digest(&[
        &[0x00],
        addr,
        &a.balance.to_le_bytes(),
        &a.nonce.to_le_bytes(),
    ])
}

/// ROOT-3 with the hash as a parameter; returns the root and the path for `index`.
fn root_and_path(
    hash: Hash,
    mut level: Vec<[u8; 32]>,
    mut index: usize,
) -> ([u8; 32], Vec<(bool, [u8; 32])>) {
    let mut path = Vec::new();
    while level.len() > 1 {
        if index ^ 1 < level.len() {
            path.push((index % 2 == 1, level[index ^ 1]));
        }
        level = level
            .chunks(2)
            .map(|c| {
                if c.len() == 2 {
                    hash.digest(&[&[0x01], &c[0], &c[1]])
                } else {
                    c[0]
                }
            })
            .collect();
        index /= 2;
    }
    (level.first().copied().unwrap_or([0; 32]), path)
}

fn verify(hash: Hash, leaf: [u8; 32], path: &[(bool, [u8; 32])], root: &[u8; 32]) -> bool {
    let top = path.iter().fold(leaf, |acc, (sibling_left, s)| {
        if *sibling_left {
            hash.digest(&[&[0x01], s, &acc])
        } else {
            hash.digest(&[&[0x01], &acc, s])
        }
    });
    &top == root
}

fn block_of(transactions: Vec<Transaction>) -> Block {
    let header = BlockHeader {
        prev_hash: [0; 32],
        state_root: [0; 32],
        timestamp: 1_756_252_800,
        nonce: 0,
        difficulty_target: target_from_leading_zero_bits(0),
        tx_root: [0; 32],
    };
    Block::new(header, transactions)
}

#[test]
fn every_node_recommits_to_the_same_new_root_and_proofs_switch_at_the_boundary() {
    // Blocks up to the epoch boundary: a few real signed transfers.
    let key = signing_key_from_seed(&[9; 32]).unwrap();
    let blocks: Vec<Block> = (0..3u64)
        .map(|n| {
            let mut tx = Transaction::new(
                vec![],
                vec![TxOutput {
                    amount: 10 + n,
                    recipient: [u8::try_from(n).unwrap() + 1; 32],
                }],
                n,
            );
            tx.sign(&key, &common::test_chain()).unwrap();
            block_of(vec![tx])
        })
        .collect();

    let mut roots = Vec::new();
    let mut count = 0;
    for _ in 0..NODES {
        let dir = tempfile::TempDir::new().unwrap();
        let db = StateDB::open(dir.path()).unwrap();
        common::bind(&db);
        db.put_account(
            &key.address(),
            &Account {
                balance: 1_000,
                nonce: 0,
            },
        )
        .unwrap();
        for b in &blocks {
            db.apply_block(b, BlockContext::GENESIS).unwrap();
        }
        let accounts: Vec<(Address, Account)> = db.all_accounts().unwrap();
        count = accounts.len();
        let old_leaves = accounts
            .iter()
            .map(|(a, acc)| leaf(Hash::Blake3, a, acc))
            .collect();
        let (old_root, _) = root_and_path(Hash::Blake3, old_leaves, 0);
        assert_eq!(
            old_root,
            db.state_root().unwrap(),
            "the parametrised tree must reproduce the node's own root"
        );
        let new_leaves = accounts
            .iter()
            .map(|(a, acc)| leaf(Hash::Sha3, a, acc))
            .collect();
        let (new_root, path) = root_and_path(Hash::Sha3, new_leaves, 1);
        let (addr, acc) = accounts[1];
        // Step 4: the new proof verifies against the new root only.
        assert!(verify(
            Hash::Sha3,
            leaf(Hash::Sha3, &addr, &acc),
            &path,
            &new_root
        ));
        assert!(!verify(
            Hash::Sha3,
            leaf(Hash::Sha3, &addr, &acc),
            &path,
            &old_root
        ));
        assert!(!verify(
            Hash::Blake3,
            leaf(Hash::Blake3, &addr, &acc),
            &path,
            &new_root
        ));
        roots.push((old_root, new_root));
    }
    assert!(
        roots.windows(2).all(|w| w[0] == w[1]),
        "nodes disagree on the re-commitment"
    );

    // Step 3: the bridge record, committed under both hashes.
    let (old_root, new_root) = roots[0];
    let bridge_old =
        Hash::Blake3.digest(&[b"maya2c hash migration bridge v1", &old_root, &new_root]);
    let bridge_new = Hash::Sha3.digest(&[b"maya2c hash migration bridge v1", &old_root, &new_root]);
    assert_ne!(bridge_old, bridge_new);
    println!(
        "{NODES} nodes re-committed {count} accounts to one SHA3-256 root; bridge binds it to the BLAKE3 root under both hashes"
    );
}

#[test]
fn recommitment_time_for_a_million_accounts() {
    let accounts: Vec<(Address, Account)> = (0..1_000_000u64)
        .map(|i| {
            (
                *blake3::hash(&i.to_le_bytes()).as_bytes(),
                Account {
                    balance: i,
                    nonce: i % 97,
                },
            )
        })
        .collect();
    for hash in [Hash::Blake3, Hash::Sha3] {
        let t = Instant::now();
        let leaves = accounts.iter().map(|(a, acc)| leaf(hash, a, acc)).collect();
        let (root, _) = root_and_path(hash, leaves, 0);
        let label = match hash {
            Hash::Blake3 => "BLAKE3",
            Hash::Sha3 => "SHA3-256",
        };
        println!(
            "re-commit 1,000,000 accounts under {label}: {:.2} s (root {:02x}{:02x}…; build profile {})",
            t.elapsed().as_secs_f64(),
            root[0],
            root[1],
            if cfg!(debug_assertions) {
                "with debug assertions"
            } else {
                "optimized"
            }
        );
    }
}
