//! The NFT reference app through the node itself (Master Prompt 30, ADR-026):
//! deployed and called by signed transactions, executed by `StateDB`, with
//! the `caller` host function reporting each transaction's real signer.
//!
//! `crates/reference-apps/tests/nft_game.rs` exercises the contract in the
//! bare VM with a caller the test chooses. This test gives the choice to the
//! node: the only way to be "Alice" is to sign as Alice.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::payload::{ContractCall, ContractDeploy, derive_contract_id};
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::state::{Account, BlockContext, StateDB};
use tempfile::TempDir;

fn wasm() -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target-contracts/wasm32-unknown-unknown/release/nft_game.wasm");
    std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e} (tracked build of contracts/nft-game)",
            path.display()
        )
    })
}

fn key(i: u8) -> HybridSigningKey {
    signing_key_from_seed(&[0x90 + i; 32]).unwrap()
}

fn block(txs: Vec<Transaction>) -> Block {
    Block::new(
        BlockHeader {
            prev_hash: [0; 32],
            state_root: [0; 32],
            timestamp: 1_760_000_000,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        txs,
    )
}

fn signed(kind: TxKind, nonce: u64, who: &HybridSigningKey) -> Transaction {
    let mut tx = Transaction::with_kind(kind, nonce);
    tx.sign(who).unwrap();
    tx
}

fn call(contract: [u8; 32], input: Vec<u8>, nonce: u64, who: &HybridSigningKey) -> Transaction {
    signed(
        TxKind::CallContract(ContractCall {
            contract,
            input,
            gas_limit: 10_000_000,
        }),
        nonce,
        who,
    )
}

fn owner_key(id: u64) -> Vec<u8> {
    let mut k = vec![b'o'; 9];
    k[1..].copy_from_slice(&id.to_le_bytes());
    k
}

#[test]
fn only_the_signing_owner_moves_a_token_through_the_node() {
    let (admin, alice, mallory, bob) = (key(0), key(1), key(2), key(3));
    let dir = TempDir::new().unwrap();
    let db = StateDB::open(dir.path()).unwrap();
    for k in [&admin, &alice, &mallory] {
        db.put_account(
            &k.address(),
            &Account {
                balance: 1_000_000,
                nonce: 0,
            },
        )
        .unwrap();
    }
    let code = wasm();
    let contract = derive_contract_id(&admin.address(), 0, &code);
    let ctx = |h| BlockContext::at_height(h);

    let mut init = vec![0u8];
    init.extend_from_slice(&admin.address());
    let mut mint = vec![1u8];
    mint.extend_from_slice(&7u64.to_le_bytes());
    mint.extend_from_slice(&alice.address());
    db.apply_block(
        &block(vec![
            signed(TxKind::DeployContract(ContractDeploy { code }), 0, &admin),
            call(contract, init, 1, &admin),
            call(contract, mint, 2, &admin),
        ]),
        ctx(1),
    )
    .unwrap();
    assert_eq!(
        db.get_contract_storage(&contract, &owner_key(7)).unwrap(),
        Some(alice.address().to_vec())
    );

    // Mallory signs a transfer of Alice's token to herself.
    let mut steal = vec![2u8];
    steal.extend_from_slice(&7u64.to_le_bytes());
    steal.extend_from_slice(&mallory.address());
    let attempt = db.apply_block(&block(vec![call(contract, steal, 0, &mallory)]), ctx(2));
    assert_eq!(
        db.get_contract_storage(&contract, &owner_key(7)).unwrap(),
        Some(alice.address().to_vec()),
        "Mallory's signed transfer moved the token ({attempt:?})"
    );

    // Alice, signing as herself, gives it to Bob.
    let mut give = vec![2u8];
    give.extend_from_slice(&7u64.to_le_bytes());
    give.extend_from_slice(&bob.address());
    db.apply_block(&block(vec![call(contract, give, 0, &alice)]), ctx(3))
        .unwrap();
    assert_eq!(
        db.get_contract_storage(&contract, &owner_key(7)).unwrap(),
        Some(bob.address().to_vec())
    );
    println!(
        "nft game on the node: deployed, minted to Alice; Mallory's signed theft {} and changed nothing; Alice's transfer to Bob landed",
        if attempt.is_err() {
            "was refused"
        } else {
            "ran"
        }
    );
}
