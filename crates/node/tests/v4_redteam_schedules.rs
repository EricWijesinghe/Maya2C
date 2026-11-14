//! Red team for scheduled payments (ADR-135 P-7, bit 46; V4-V1). Each test
//! is one attack attempt against real block execution, named for the attack,
//! asserting the refusal and that the native total (payer + payee + fee
//! collector) never moved. The threat-model replays (S-1..S-14) are in
//! `scheduled_payment_tests.rs`; this file is refund double-spends, nonce
//! replays and governance changing `RUN_FEE` under a live schedule.
//!
//! Classes tried (V4-V1 brief): value creation (refunds), fee bypass (a
//! lowered `RUN_FEE` is refunded, never kept or skipped), unbounded per-block
//! work (pinned there: the 257th run carries over), replay, squatting (the id
//! is bound to payer and nonce).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use custom_l1_node::core::schedule_payload::ScheduleAction;
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::NodeError;
use custom_l1_node::governance::ParameterTable;
use custom_l1_node::state::fees::FEE_COLLECTOR;
use custom_l1_node::state::scheduled::{MIN_INTERVAL, schedule_id};
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};
use maya_governance::params::ParameterKey;
use primitive_types::U256;
use tempfile::TempDir;

mod common;

const V4: u64 = 10;
const FUNDS: u64 = 1_000_000;
const AMOUNT: u64 = 100;
const FEE: u64 = 7;
const PAYEE: Address = [0x43; 32];

struct Net {
    db: StateDB,
    _dir: TempDir,
}

fn net(funded: &[Address], run_fee: u64) -> Net {
    let dir = TempDir::new().unwrap();
    let db = StateDB::open(dir.path()).unwrap();
    common::bind(&db);
    for a in funded {
        db.put_account(
            a,
            &Account {
                balance: U256::from(FUNDS),
                nonce: 0,
            },
        )
        .unwrap();
    }
    let n = Net { db, _dir: dir };
    set_run_fee(&n, run_fee);
    n
}

/// What a passed proposal would leave: the governed `RUN_FEE` in state.
fn set_run_fee(n: &Net, fee: u64) {
    let table = ParameterTable::new().with(
        ParameterKey::ScheduleRunFee,
        maya_dex::Amount::from_u64(fee),
    );
    n.db.raw_put_for_test(b"g:params", &table.encode()).unwrap();
}

fn apply(n: &Net, txs: Vec<Transaction>, height: u64) -> Result<[u8; 32], NodeError> {
    let block = Block::new(
        BlockHeader {
            version: 0,
            prev_hash: [0; 32],
            state_root: [0; 32],
            timestamp: 1_756_252_800,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        txs,
    );
    n.db.apply_block(
        &block,
        BlockContext::at_height(height).with_scheduled_activation(V4),
    )
}

fn signed(kind: TxKind, nonce: u64, key: &HybridSigningKey) -> Transaction {
    let mut tx = Transaction::with_kind(kind, nonce);
    tx.sign(key, &common::test_chain()).unwrap();
    tx
}

fn create(runs: u32) -> TxKind {
    TxKind::Schedule(ScheduleAction::Create {
        payee: PAYEE,
        amount: U256::from(AMOUNT),
        first_due: V4 + 5,
        interval: MIN_INTERVAL,
        runs,
    })
}

fn cancel(id: [u8; 32]) -> TxKind {
    TxKind::Schedule(ScheduleAction::Cancel { schedule_id: id })
}

fn balance(n: &Net, who: &Address) -> U256 {
    n.db.get_account(who).unwrap().balance
}

/// Payer, payee and collector together: the escrow is outside all three, so
/// this equals `FUNDS` exactly when no schedule holds anything.
fn outside_escrow(n: &Net, payer: &Address) -> U256 {
    balance(n, payer) + balance(n, &PAYEE) + balance(n, &FEE_COLLECTOR)
}

fn payer(seed: u8) -> (HybridSigningKey, Address) {
    let key = signing_key_from_seed(&[seed; 32]).unwrap();
    let address = key.address();
    (key, address)
}

// Value creation: cancel the same schedule twice in one block, hoping the
// second close reads the committed record and refunds the escrow again.
// The second cancel sees the first's deletion, the block is refused whole,
// and on its own the first cancel refunds once.
#[test]
fn a_double_cancel_in_one_block_refunds_once() {
    let (key, me) = payer(0x81);
    let n = net(&[me], FEE);
    apply(&n, vec![signed(create(3), 0, &key)], V4).unwrap();
    let id = schedule_id(&me, 0);
    let escrow = U256::from(3 * (AMOUNT + FEE));
    assert_eq!(balance(&n, &me), U256::from(FUNDS) - escrow);

    let first = signed(cancel(id), 1, &key);
    let second = signed(cancel(id), 2, &key);
    let err = apply(&n, vec![first.clone(), second], V4 + 1).unwrap_err();
    assert!(err.to_string().contains("no such schedule"), "{err}");
    assert_eq!(
        balance(&n, &me),
        U256::from(FUNDS) - escrow,
        "nothing applied"
    );

    apply(&n, vec![first.clone()], V4 + 1).expect("one cancel");
    assert_eq!(balance(&n, &me), U256::from(FUNDS));
    // Replayed later: refused on the nonce, so no second refund either.
    let err = apply(&n, vec![first], V4 + 2).unwrap_err();
    assert!(matches!(err, NodeError::InvalidNonce { .. }), "{err:?}");
    assert_eq!(balance(&n, &me), U256::from(FUNDS));
    assert_eq!(n.db.get_schedule(&id).unwrap(), None);
}

// Value creation after a run: cancel refunds exactly what the runs left,
// and the native total outside escrow is back to FUNDS.
#[test]
fn a_cancel_after_a_run_refunds_exactly_the_unspent_escrow() {
    let (key, me) = payer(0x82);
    let n = net(&[me], FEE);
    apply(&n, vec![signed(create(3), 0, &key)], V4).unwrap();
    let id = schedule_id(&me, 0);
    apply(&n, vec![], V4 + 5).expect("first run");
    assert_eq!(balance(&n, &PAYEE), U256::from(AMOUNT));
    assert_eq!(balance(&n, &FEE_COLLECTOR), U256::from(FEE));

    apply(&n, vec![signed(cancel(id), 1, &key)], V4 + 6).unwrap();
    let spent = U256::from(AMOUNT + FEE);
    assert_eq!(balance(&n, &me), U256::from(FUNDS) - spent);
    assert_eq!(outside_escrow(&n, &me), U256::from(FUNDS));
}

// Fee bypass in reverse: governance lowers RUN_FEE under a live schedule.
// The prepaid difference must come back to the payer at close, neither kept
// in a deleted record nor paid to the collector.
#[test]
fn a_lowered_run_fee_is_refunded_at_close_and_value_is_conserved() {
    let (key, me) = payer(0x83);
    let n = net(&[me], FEE);
    apply(&n, vec![signed(create(2), 0, &key)], V4).unwrap();
    let id = schedule_id(&me, 0);
    apply(&n, vec![], V4 + 5).expect("run at the old fee");
    set_run_fee(&n, 0);
    apply(&n, vec![], V4 + 5 + MIN_INTERVAL).expect("last run at zero fee");

    assert_eq!(n.db.get_schedule(&id).unwrap(), None, "closed");
    assert_eq!(balance(&n, &PAYEE), U256::from(2 * AMOUNT));
    assert_eq!(
        balance(&n, &FEE_COLLECTOR),
        U256::from(FEE),
        "one run's fee"
    );
    assert_eq!(
        balance(&n, &me),
        U256::from(FUNDS - 2 * AMOUNT - FEE),
        "the second run's prepaid fee came back"
    );
    assert_eq!(outside_escrow(&n, &me), U256::from(FUNDS));
}

// Replay and squatting: the same signed create, offered again, is refused
// on its nonce (no second escrow debit); the payer's next create gets a new
// id, and a stranger cannot cancel either.
#[test]
fn a_replayed_create_is_refused_and_ids_cannot_be_taken_by_a_stranger() {
    let (key, me) = payer(0x84);
    let (thief, _) = payer(0x85);
    let n = net(&[me, thief.address()], FEE);
    let tx = signed(create(1), 0, &key);
    apply(&n, vec![tx.clone()], V4).unwrap();
    let err = apply(&n, vec![tx], V4 + 1).unwrap_err();
    assert!(matches!(err, NodeError::InvalidNonce { .. }), "{err:?}");
    assert_eq!(balance(&n, &me), U256::from(FUNDS - (AMOUNT + FEE)));

    apply(&n, vec![signed(create(1), 1, &key)], V4 + 1).unwrap();
    let (first, second) = (schedule_id(&me, 0), schedule_id(&me, 1));
    assert_ne!(first, second);
    for id in [first, second] {
        let err = apply(&n, vec![signed(cancel(id), 0, &thief)], V4 + 2).unwrap_err();
        assert!(err.to_string().contains("only the payer"), "{err}");
    }
    // The thief's own create at its nonce 0 derives from its own address.
    assert_ne!(schedule_id(&thief.address(), 0), first);
}
