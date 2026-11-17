//! v3 P1: block application with its parallel phase on gives exactly what it
//! gives with the phase off.
//!
//! Two state databases receive the same blocks. One applies them as every node
//! does today; the other has `BlockContext::parallel_apply_activation` at zero,
//! so `stage_block` verifies every signature on every core before the
//! sequential pass. Blocks are drawn from a fixed seed: conflicting ones (a few
//! senders with several transactions each, paying two hot accounts and each
//! other) and non-conflicting ones (one transaction per sender, fresh
//! recipients). Before each valid block is applied, three refused variants of
//! it are applied too — a forged signature, a replayed transaction and an
//! overspend — and both databases must refuse each with the same error and keep
//! the same state.
//!
//! What would make this fail: the parallel phase caching a failed verification
//! (the forged variant would then be accepted on one side only), or it
//! touching state (the roots would part).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::ChainTag;
use custom_l1_node::core::block::{Block, BlockHeader};
use custom_l1_node::core::transaction::{Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::state::context::PARALLEL_APPLY_ACTIVATION_HEIGHT;
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};
use primitive_types::U256;
use rand_chacha::ChaCha20Rng;
use rand_chacha::rand_core::{RngCore, SeedableRng};
use tempfile::TempDir;

mod common;

/// The draw. Fixed so a failure replays exactly.
const SEED: u64 = 0x5EED_0F_0001;
const SENDERS: usize = 72;
/// Below 64 transactions `prewarm_verification` runs inline (32 per thread),
/// so every block here is at least that big for the phase to be parallel.
const MIN_TXS: usize = 64;
const EXTRA_TXS: u64 = 9;
const ROUNDS: usize = 6;
/// Senders a conflicting block draws from.
const CONFLICT_SENDERS: u64 = 8;
const HOT: [Address; 2] = [[0xA1; 32], [0xA2; 32]];
const FUNDS: u64 = 1_000_000;
const MAX_PAYMENT: u64 = 50;

/// What a transaction will be, decided before it is signed.
#[derive(Clone, Copy)]
struct Spec {
    sender: usize,
    nonce: u64,
    recipient: Address,
    amount: u64,
}

struct Ledger {
    db: StateDB,
    _dir: TempDir,
}

impl Ledger {
    fn funded(keys: &[HybridSigningKey]) -> Self {
        let dir = TempDir::new().unwrap();
        let db = StateDB::open(dir.path()).unwrap();
        common::bind(&db);
        for k in keys {
            let account = Account {
                balance: U256::from(FUNDS),
                nonce: 0,
            };
            db.put_account(&k.address(), &account).unwrap();
        }
        Self { db, _dir: dir }
    }
}

fn block_of(height: u64, txs: Vec<Transaction>) -> Block {
    let header = BlockHeader {
        version: 0,
        prev_hash: [0; 32],
        state_root: [0; 32],
        timestamp: 1_790_000_000 + height,
        nonce: 0,
        difficulty_target: target_from_leading_zero_bits(0),
        tx_root: [0; 32],
    };
    Block::new(header, txs)
}

fn sign(spec: Spec, keys: &[HybridSigningKey], chain: &ChainTag) -> Transaction {
    let output = TxOutput {
        amount: U256::from(spec.amount),
        recipient: spec.recipient,
    };
    let mut tx = Transaction::new(vec![], vec![output], spec.nonce);
    tx.sign(&keys[spec.sender], chain).unwrap();
    tx
}

/// Signs on every core; signing is the slow part of this test, not what it checks.
fn sign_all(specs: &[Spec], keys: &[HybridSigningKey], chain: &ChainTag) -> Vec<Transaction> {
    let cores = std::thread::available_parallelism().map_or(1, usize::from);
    let chunk = specs.len().div_ceil(cores).max(1);
    std::thread::scope(|s| {
        let handles: Vec<_> = specs
            .chunks(chunk)
            .map(|part| {
                s.spawn(move || {
                    part.iter()
                        .map(|sp| sign(*sp, keys, chain))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect()
    })
}

fn below(rng: &mut ChaCha20Rng, n: u64) -> u64 {
    rng.next_u64() % n
}

/// One block's specs. Advances `nonces` as if the block will apply.
fn draw(
    rng: &mut ChaCha20Rng,
    keys: &[HybridSigningKey],
    nonces: &mut [u64],
    round: usize,
) -> Vec<Spec> {
    let len = MIN_TXS + usize::try_from(below(rng, EXTRA_TXS)).unwrap();
    let conflicting = below(rng, 2) == 0;
    (0..len)
        .map(|i| {
            let (sender, recipient) = if conflicting {
                let sender = usize::try_from(below(rng, CONFLICT_SENDERS)).unwrap();
                // Half pay a hot account, half pay another sender in this block:
                // a read the payee's own later transaction depends on.
                let recipient = if below(rng, 2) == 0 {
                    HOT[usize::try_from(below(rng, 2)).unwrap()]
                } else {
                    keys[usize::try_from(below(rng, CONFLICT_SENDERS)).unwrap()].address()
                };
                (sender, recipient)
            } else {
                let mut fresh = [0u8; 32];
                fresh[..8].copy_from_slice(&u64::try_from(round).unwrap().to_le_bytes());
                fresh[8..16].copy_from_slice(&u64::try_from(i).unwrap().to_le_bytes());
                fresh[31] = 0xF0;
                (i, fresh)
            };
            let nonce = nonces[sender];
            nonces[sender] += 1;
            Spec {
                sender,
                nonce,
                recipient,
                amount: 1 + below(rng, MAX_PAYMENT),
            }
        })
        .collect()
}

/// Applies `block` at `height` to both ledgers and requires the same outcome.
fn both(
    serial: &Ledger,
    parallel: &Ledger,
    height: u64,
    block: &Block,
) -> Result<[u8; 32], String> {
    let off = BlockContext::at_height(height);
    let on = off.with_parallel_apply_activation(0);
    assert!(!off.parallel_apply_active() && on.parallel_apply_active());
    let a = serial
        .db
        .apply_block(block, off)
        .map_err(|e| format!("{e:?}"));
    let b = parallel
        .db
        .apply_block(block, on)
        .map_err(|e| format!("{e:?}"));
    assert_eq!(
        a, b,
        "parallel apply diverged from serial at height {height}"
    );
    a
}

#[test]
fn parallel_and_serial_apply_give_one_root_over_random_blocks() {
    let chain = common::test_chain();
    let keys: Vec<HybridSigningKey> = (0..SENDERS)
        .map(|i| {
            let mut seed = [0x5Au8; 32];
            seed[..8].copy_from_slice(&u64::try_from(i).unwrap().to_le_bytes());
            signing_key_from_seed(&seed).unwrap()
        })
        .collect();
    let serial = Ledger::funded(&keys);
    let parallel = Ledger::funded(&keys);
    let mut rng = ChaCha20Rng::seed_from_u64(SEED);
    let mut nonces = vec![0u64; SENDERS];
    let mut applied = 0usize;

    for round in 0..ROUNDS {
        let height = u64::try_from(round).unwrap() + 1;
        let specs = draw(&mut rng, &keys, &mut nonces, round);
        let txs = sign_all(&specs, &keys, &chain);
        let pick = usize::try_from(below(&mut rng, u64::try_from(txs.len()).unwrap())).unwrap();

        // Forged: one bit of one lattice signature flipped.
        let mut forged = txs.clone();
        forged[pick].signature.as_mut().unwrap().lattice[0] ^= 1;
        let err = both(&serial, &parallel, height, &block_of(height, forged)).unwrap_err();
        assert!(
            err.contains("Signature") || err.contains("signature"),
            "{err}"
        );

        // Replayed: the first transaction again at the end.
        let mut replayed = txs.clone();
        replayed.push(txs[0].clone());
        let err = both(&serial, &parallel, height, &block_of(height, replayed)).unwrap_err();
        assert!(err.contains("InvalidNonce"), "{err}");

        // Overspend: the same sender and nonce, for more than it holds.
        let mut over = txs.clone();
        let big = Spec {
            amount: FUNDS * 2,
            ..specs[pick]
        };
        over[pick] = sign(big, &keys, &chain);
        let err = both(&serial, &parallel, height, &block_of(height, over)).unwrap_err();
        assert!(err.contains("InsufficientBalance"), "{err}");

        // The block itself, after three refusals that must have left no trace.
        both(&serial, &parallel, height, &block_of(height, txs)).expect("the valid block applies");
        applied += specs.len();
    }
    println!(
        "parallel_apply differential: {ROUNDS} blocks, {applied} transactions, seed {SEED:#x}"
    );
}

#[test]
fn the_parallel_phase_is_dark_by_default() {
    assert_eq!(PARALLEL_APPLY_ACTIVATION_HEIGHT, u64::MAX);
    assert!(!BlockContext::at_height(u64::MAX - 1).parallel_apply_active());
    assert!(!BlockContext::GENESIS.parallel_apply_active());
}
