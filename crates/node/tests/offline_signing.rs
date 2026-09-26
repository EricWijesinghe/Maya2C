//! The air-gapped signing loop (Master Prompt 17 §2), on real node types:
//! build → export unsigned → sign offline → export signed → broadcast.
//!
//! The only thing that crosses the air gap is bytes: the online machine never
//! holds the key, the offline machine never touches state or the network.
//! "Broadcast" is mempool admission plus block application on a real
//! `StateDB`, which is what a node does with a transaction it receives.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use custom_l1_node::core::{Block, BlockHeader, Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::signing_key_from_seed;
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::network::mempool::Mempool;
use custom_l1_node::state::{Account, BlockContext, StateDB};

/// Online side: knows the sender's public address and nonce, not the key.
fn build_unsigned(nonce: u64, to: [u8; 32], amount: u64) -> Vec<u8> {
    Transaction::new(
        vec![],
        vec![TxOutput {
            amount,
            recipient: to,
        }],
        nonce,
    )
    .to_bytes()
}

/// Offline side: decodes, shows the operator what it signs, signs, re-encodes.
fn sign_offline(unsigned: &[u8], seed: &[u8; 32]) -> (Vec<u8>, String) {
    let mut tx = Transaction::from_bytes(unsigned)
        .expect("the offline side refuses a frame it cannot decode");
    assert!(
        tx.signature.is_none(),
        "the offline side is handed an unsigned frame"
    );
    let summary = format!(
        "nonce {} pays {} to {}",
        tx.nonce,
        tx.outputs[0].amount,
        hex::encode(tx.outputs[0].recipient)
    );
    tx.sign(&signing_key_from_seed(seed).unwrap()).unwrap();
    (tx.to_bytes(), summary)
}

#[test]
fn build_export_sign_offline_and_broadcast() {
    let seed = [42u8; 32];
    let sender = signing_key_from_seed(&seed).unwrap().address(); // the online side is told this once
    let dir = tempfile::TempDir::new().unwrap();
    let state = Arc::new(StateDB::open(dir.path()).unwrap());
    state
        .put_account(
            &sender,
            &Account {
                balance: 1_000,
                nonce: 0,
            },
        )
        .unwrap();

    let unsigned = build_unsigned(0, [7; 32], 250);
    let (signed, summary) = sign_offline(&unsigned, &seed);
    assert_eq!(
        summary,
        format!("nonce 0 pays 250 to {}", hex::encode([7u8; 32]))
    );

    // Broadcast: the receiving node decodes, admits to its mempool, and applies.
    let pool = Mempool::new(Arc::clone(&state));
    assert!(
        pool.insert_encoded(&signed).unwrap(),
        "the mempool must admit the offline-signed transaction"
    );
    let tx = Transaction::from_bytes(&signed).unwrap();
    assert_eq!(
        tx.sender(),
        sender,
        "the signature binds the offline key's address"
    );
    let header = BlockHeader {
        prev_hash: [0; 32],
        state_root: [0; 32],
        timestamp: 1_756_252_800,
        nonce: 0,
        difficulty_target: target_from_leading_zero_bits(0),
        tx_root: [0; 32],
    };
    state
        .apply_block(&Block::new(header, vec![tx]), BlockContext::GENESIS)
        .unwrap();
    assert_eq!(
        state.get_account(&sender).unwrap(),
        Account {
            balance: 750,
            nonce: 1
        }
    );
    assert_eq!(state.get_account(&[7; 32]).unwrap().balance, 250);
}

#[test]
fn a_frame_altered_after_signing_is_refused() {
    let seed = [43u8; 32];
    let sender = signing_key_from_seed(&seed).unwrap().address();
    let dir = tempfile::TempDir::new().unwrap();
    let state = Arc::new(StateDB::open(dir.path()).unwrap());
    state
        .put_account(
            &sender,
            &Account {
                balance: 1_000,
                nonce: 0,
            },
        )
        .unwrap();
    let (mut signed, _) = sign_offline(&build_unsigned(0, [7; 32], 250), &seed);
    // Change the amount on the way back across the gap (bytes 17..25 are the first output's amount).
    signed[17] ^= 0x01;
    let pool = Mempool::new(Arc::clone(&state));
    assert!(
        pool.insert_encoded(&signed).is_err(),
        "a tampered frame must not be admitted"
    );
}
