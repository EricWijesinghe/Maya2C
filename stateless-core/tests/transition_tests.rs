//! Transfers against a verified witness, by the node's rules.

use maya_stateless_core::{
    AccountState, Blake3, Defect, Error, Key, Value, VerifiedTree, Violation, apply_transfer,
    sparse,
};

fn key(byte: u8) -> Key {
    *blake3::hash(&[byte]).as_bytes()
}

fn account(balance: u64, nonce: u64) -> Value {
    AccountState { balance, nonce }.encode()
}

fn state(tree: &VerifiedTree<[u8; 32]>, key: &Key) -> AccountState {
    tree.get(key)
        .expect("opened")
        .map(|value| AccountState::decode(&value))
        .unwrap_or_default()
}

fn tree(accounts: &[(Key, Value)], touched: &[Key]) -> VerifiedTree<[u8; 32]> {
    let mut set = accounts.to_vec();
    set.sort_unstable_by_key(|leaf| leaf.0);
    let root = sparse::root(&Blake3, &set).expect("root");
    sparse::open(&Blake3, &set, touched)
        .expect("open")
        .verify(&Blake3, |digest| *digest == root)
        .expect("verify")
}

#[test]
fn the_account_encoding_is_balance_then_nonce_little_endian() {
    let encoded = AccountState {
        balance: 1,
        nonce: 2,
    }
    .encode();
    assert_eq!(encoded[..8], 1u64.to_le_bytes());
    assert_eq!(encoded[8..], 2u64.to_le_bytes());
    assert_eq!(
        AccountState::decode(&encoded),
        AccountState {
            balance: 1,
            nonce: 2
        }
    );
}

#[test]
fn a_transfer_debits_credits_creates_and_bumps_the_nonce() {
    let (alice, bob, carol) = (key(1), key(2), key(3));
    let mut tree = tree(
        &[(alice, account(100, 4)), (bob, account(5, 0))],
        &[alice, bob, carol],
    );

    apply_transfer(&mut tree, alice, 4, [(bob, 30), (carol, 20)]).expect("apply");

    assert_eq!(
        state(&tree, &alice),
        AccountState {
            balance: 50,
            nonce: 5
        }
    );
    assert_eq!(
        state(&tree, &bob),
        AccountState {
            balance: 35,
            nonce: 0
        }
    );
    assert_eq!(
        state(&tree, &carol),
        AccountState {
            balance: 20,
            nonce: 0
        }
    );
}

#[test]
fn a_self_transfer_reads_the_debited_balance_back() {
    let alice = key(1);
    let mut tree = tree(&[(alice, account(10, 0))], &[alice]);
    apply_transfer(&mut tree, alice, 0, [(alice, 10)]).expect("apply");
    assert_eq!(
        state(&tree, &alice),
        AccountState {
            balance: 10,
            nonce: 1
        }
    );
}

#[test]
fn a_transfer_with_no_outputs_still_spends_the_nonce() {
    let alice = key(1);
    let mut tree = tree(&[(alice, account(10, 0))], &[alice]);
    apply_transfer(&mut tree, alice, 0, []).expect("apply");
    assert_eq!(
        state(&tree, &alice),
        AccountState {
            balance: 10,
            nonce: 1
        }
    );
}

#[test]
fn a_sender_that_does_not_exist_reads_as_zero_and_can_send_nothing() {
    let (ghost, bob) = (key(9), key(2));
    let mut tree = tree(&[], &[ghost, bob]);
    apply_transfer(&mut tree, ghost, 0, []).expect("zero-value transfer");
    assert_eq!(
        apply_transfer(&mut tree, ghost, 1, [(bob, 1)]),
        Err(Error::Invalid(Violation::InsufficientBalance {
            key: ghost,
            required: 1,
            available: 0,
        }))
    );
}

#[test]
fn a_wrong_nonce_is_a_violation() {
    let (alice, bob) = (key(1), key(2));
    let mut tree = tree(&[(alice, account(10, 3))], &[alice, bob]);
    let error = apply_transfer(&mut tree, alice, 2, [(bob, 1)]).expect_err("nonce");
    assert!(error.is_invalid());
    assert_eq!(
        error,
        Error::Invalid(Violation::Nonce {
            key: alice,
            expected: 3,
            actual: 2
        })
    );
}

#[test]
fn an_output_total_or_a_credit_past_u64_is_a_violation() {
    let (alice, bob) = (key(1), key(2));
    let mut tree = tree(
        &[(alice, account(u64::MAX, 0)), (bob, account(u64::MAX, 0))],
        &[alice, bob],
    );
    assert_eq!(
        apply_transfer(&mut tree, alice, 0, [(bob, u64::MAX), (bob, 1)]),
        Err(Error::Invalid(Violation::Overflow { key: alice }))
    );
    assert_eq!(
        apply_transfer(&mut tree, alice, 0, [(bob, 1)]),
        Err(Error::Invalid(Violation::Overflow { key: bob }))
    );
}

#[test]
fn a_recipient_the_witness_does_not_open_is_unverifiable_not_invalid() {
    let (alice, bob) = (key(1), key(2));
    let others: Vec<(Key, Value)> = (10..80).map(|byte| (key(byte), account(1, 0))).collect();
    let mut accounts = others.clone();
    accounts.push((alice, account(10, 0)));
    let mut tree = tree(&accounts, &[alice]);

    let error = apply_transfer(&mut tree, alice, 0, [(bob, 1)]).expect_err("unopened");
    assert!(!error.is_invalid());
    assert_eq!(error, Error::Unverifiable(Defect::Unopened { key: bob }));
}
