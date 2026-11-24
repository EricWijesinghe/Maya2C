//! Matching threats: per-invoice deposit addresses, exact amounts, double use,
//! timing windows and finality. Pure logic, no network.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_checkout::chain::{BlockOutputs, Finalized, Output, TxOutputs};
use maya_checkout::invoice::{Delivery, Invoice, NewInvoice, Status, UnmatchedReason};
use maya_checkout::matching::{UNMATCHED_CAP, confirm, detect, expire, roll_back};
use maya_checkout::store::{Claim, Ledger, claim_key};
use primitive_types::U256;

/// Deposit addresses: one per invoice, never shared.
const ADDR_A: [u8; 32] = [9; 32];
const ADDR_B: [u8; 32] = [8; 32];
/// In the merchant's pool, assigned to no invoice yet.
const ADDR_FREE: [u8; 32] = [7; 32];
/// Not the merchant's at all.
const OTHER: [u8; 32] = [4; 32];

fn invoice_at(
    id: &str,
    recipient: [u8; 32],
    amount: u64,
    created: u64,
    expires_in: u64,
) -> Invoice {
    let req = NewInvoice {
        amount: U256::from(amount),
        memo: id.into(),
        expires_in_blocks: expires_in,
        webhook_url: "https://93.184.216.34/hook".into(),
    };
    Invoice::open(id.into(), recipient, &req, created).unwrap()
}

fn invoice(id: &str, amount: u64, created: u64, expires_in: u64) -> Invoice {
    invoice_at(id, ADDR_A, amount, created, expires_in)
}

fn ledger_with(invoices: Vec<Invoice>) -> Ledger {
    let mut ledger = Ledger {
        invoices,
        ..Ledger::default()
    };
    for a in [ADDR_A, ADDR_B, ADDR_FREE] {
        ledger.addresses.insert(hex::encode(a));
    }
    ledger
}

/// A block whose id is `b<height>` and whose parent is `b<height-1>`.
fn block(height: u64, txid: &str, recipient: [u8; 32], amount: u64) -> BlockOutputs {
    BlockOutputs {
        height,
        id: format!("b{height}"),
        prev_id: format!("b{}", height.saturating_sub(1)),
        txs: vec![TxOutputs {
            txid: txid.into(),
            outputs: vec![Output {
                recipient,
                amount: U256::from(amount),
            }],
        }],
    }
}

fn fin(height: u64) -> Finalized {
    Finalized {
        height,
        block_id: format!("b{height}"),
    }
}

fn status_of(ledger: &Ledger, id: &str) -> Status {
    ledger.invoices.iter().find(|i| i.id == id).unwrap().status
}

fn settled_by(ledger: &Ledger, txid: &str) -> Option<String> {
    match ledger.claims.get(&claim_key(txid, 0)) {
        Some(Claim::Invoice(id)) => Some(id.clone()),
        _ => None,
    }
}

#[test]
fn a_payment_to_one_invoices_address_never_settles_another_invoice() {
    // The reviewed attack: invoice `a` asks 500 at ADDR_A, invoice `b` asks 600
    // at ADDR_B. 600 sent to ADDR_A is `a`'s overpayment; it must not settle `b`
    // even though it is exactly `b`'s amount.
    let mut ledger = ledger_with(vec![
        invoice_at("a", ADDR_A, 500, 0, 50),
        invoice_at("b", ADDR_B, 600, 0, 50),
    ]);
    assert!(detect(&mut ledger, &block(5, "tx1", ADDR_A, 600)).is_empty());
    assert_eq!(status_of(&ledger, "a"), Status::Pending);
    assert_eq!(status_of(&ledger, "b"), Status::Pending);
    assert_eq!(ledger.unmatched[0].reason, UnmatchedReason::Overpaid);
    assert_eq!(ledger.unmatched[0].invoice_id.as_deref(), Some("a"));
}

#[test]
fn two_invoices_with_one_amount_are_told_apart_by_address() {
    let mut ledger = ledger_with(vec![
        invoice_at("a", ADDR_A, 500, 0, 50),
        invoice_at("b", ADDR_B, 500, 0, 50),
    ]);
    assert_eq!(
        detect(&mut ledger, &block(5, "tx1", ADDR_B, 500)),
        vec!["b".to_owned()]
    );
    assert_eq!(status_of(&ledger, "a"), Status::Pending);
    assert_eq!(status_of(&ledger, "b"), Status::Detected);
}

#[test]
fn underpayment_is_recorded_against_its_invoice_and_settles_nothing() {
    let mut ledger = ledger_with(vec![invoice("a", 1000, 0, 50)]);
    assert!(detect(&mut ledger, &block(5, "tx1", ADDR_A, 999)).is_empty());
    assert_eq!(status_of(&ledger, "a"), Status::Pending);
    assert_eq!(ledger.unmatched.len(), 1);
    assert_eq!(ledger.unmatched[0].amount, U256::from(999u64));
    assert_eq!(ledger.unmatched[0].reason, UnmatchedReason::Underpaid);
    assert_eq!(ledger.unmatched[0].invoice_id.as_deref(), Some("a"));
    assert_eq!(
        ledger.claims.get(&claim_key("tx1", 0)),
        Some(&Claim::Unmatched)
    );
}

#[test]
fn overpayment_is_recorded_and_never_settles() {
    let mut ledger = ledger_with(vec![invoice("a", 1000, 0, 50)]);
    assert!(detect(&mut ledger, &block(5, "tx1", ADDR_A, 1500)).is_empty());
    assert_eq!(status_of(&ledger, "a"), Status::Pending);
    assert_eq!(ledger.unmatched[0].reason, UnmatchedReason::Overpaid);
}

#[test]
fn a_second_payment_to_a_settled_invoice_is_recorded_not_applied() {
    let mut ledger = ledger_with(vec![invoice("a", 500, 0, 50)]);
    assert_eq!(
        detect(&mut ledger, &block(5, "tx1", ADDR_A, 500)),
        vec!["a".to_owned()]
    );
    assert!(detect(&mut ledger, &block(6, "tx2", ADDR_A, 500)).is_empty());
    assert_eq!(ledger.unmatched[0].reason, UnmatchedReason::AlreadySettled);
    assert_eq!(settled_by(&ledger, "tx1").as_deref(), Some("a"));
}

#[test]
fn a_payment_to_an_unassigned_pool_address_is_recorded() {
    let mut ledger = ledger_with(vec![invoice("a", 500, 0, 50)]);
    assert!(detect(&mut ledger, &block(5, "tx1", ADDR_FREE, 500)).is_empty());
    assert_eq!(status_of(&ledger, "a"), Status::Pending);
    assert_eq!(ledger.unmatched[0].reason, UnmatchedReason::Unassigned);
    assert!(ledger.unmatched[0].invoice_id.is_none());
}

#[test]
fn one_output_is_claimed_once_even_when_the_block_repeats() {
    let mut ledger = ledger_with(vec![invoice("a", 500, 0, 50)]);
    let b5 = block(5, "tx1", ADDR_A, 500);
    assert_eq!(detect(&mut ledger, &b5), vec!["a".to_owned()]);
    // The same block seen again (a re-scan after a crash) must not claim again.
    assert!(detect(&mut ledger, &b5).is_empty());
    assert_eq!(settled_by(&ledger, "tx1").as_deref(), Some("a"));
    assert_eq!(ledger.unmatched.len(), 0);
}

#[test]
fn payment_in_or_before_the_creation_block_is_not_settled() {
    // Created with tip 10: only blocks above 10 can pay.
    let mut ledger = ledger_with(vec![invoice("a", 500, 10, 50)]);
    assert!(detect(&mut ledger, &block(10, "old", ADDR_A, 500)).is_empty());
    assert!(detect(&mut ledger, &block(9, "older", ADDR_A, 500)).is_empty());
    assert_eq!(ledger.unmatched[0].reason, UnmatchedReason::Early);
    assert_eq!(
        detect(&mut ledger, &block(11, "new", ADDR_A, 500)),
        vec!["a".to_owned()]
    );
}

#[test]
fn payment_after_the_window_is_not_settled() {
    let mut ledger = ledger_with(vec![invoice("a", 500, 0, 10)]);
    assert!(detect(&mut ledger, &block(11, "late", ADDR_A, 500)).is_empty());
    assert_eq!(ledger.unmatched[0].reason, UnmatchedReason::Late);
    assert_eq!(
        detect(&mut ledger, &block(10, "last", ADDR_A, 500)),
        vec!["a".to_owned()]
    );
}

#[test]
fn expiry_happens_only_after_the_window_is_scanned() {
    let mut ledger = ledger_with(vec![invoice("a", 500, 0, 10)]);
    assert!(expire(&mut ledger, 9).is_empty());
    assert_eq!(expire(&mut ledger, 10), vec!["a".to_owned()]);
    assert_eq!(status_of(&ledger, "a"), Status::Expired);
}

#[test]
fn payment_to_another_address_is_ignored_not_recorded() {
    let mut ledger = ledger_with(vec![invoice("a", 500, 0, 50)]);
    assert!(detect(&mut ledger, &block(5, "tx1", OTHER, 500)).is_empty());
    assert!(ledger.unmatched.is_empty());
    assert!(ledger.claims.is_empty());
}

#[test]
fn only_finality_confirms_and_queues_the_webhook() {
    let mut ledger = ledger_with(vec![invoice("a", 500, 0, 50)]);
    detect(&mut ledger, &block(20, "tx1", ADDR_A, 500));
    for h in 18..=22 {
        ledger.block_ids.insert(h, format!("b{h}"));
    }
    // No checkpoint, or one below the payment: not final, however deep it is.
    assert!(confirm(&mut ledger, None, 1000).is_empty());
    assert!(confirm(&mut ledger, Some(&fin(19)), 1000).is_empty());
    assert_eq!(status_of(&ledger, "a"), Status::Detected);
    assert_eq!(
        confirm(&mut ledger, Some(&fin(21)), 1000),
        vec!["a".to_owned()]
    );
    let inv = &ledger.invoices[0];
    assert_eq!(inv.status, Status::Confirmed);
    assert_eq!(inv.delivery, Delivery::Due);
    assert_eq!(inv.next_attempt_unix, 1000);
}

#[test]
fn a_checkpoint_on_another_branch_confirms_nothing() {
    let mut ledger = ledger_with(vec![invoice("a", 500, 0, 50)]);
    detect(&mut ledger, &block(20, "tx1", ADDR_A, 500));
    ledger.block_ids.insert(20, "b20".into());
    ledger.block_ids.insert(21, "b21".into());
    let elsewhere = Finalized {
        height: 21,
        block_id: "x21".into(),
    };
    assert!(confirm(&mut ledger, Some(&elsewhere), 1000).is_empty());
    assert_eq!(status_of(&ledger, "a"), Status::Detected);
}

#[test]
fn a_checkpoint_above_the_scanned_blocks_confirms_nothing() {
    let mut ledger = ledger_with(vec![invoice("a", 500, 0, 50)]);
    detect(&mut ledger, &block(20, "tx1", ADDR_A, 500));
    ledger.block_ids.insert(20, "b20".into());
    // Height 30 is not scanned, so its id cannot be checked against this chain.
    assert!(confirm(&mut ledger, Some(&fin(30)), 1000).is_empty());
}

#[test]
fn confirm_reverts_a_payment_whose_block_id_changed() {
    let mut ledger = ledger_with(vec![invoice("a", 500, 0, 50)]);
    detect(&mut ledger, &block(20, "tx1", ADDR_A, 500));
    // The node now has a different block at height 20: the payment is gone.
    ledger.block_ids.insert(20, "forked".into());
    assert!(confirm(&mut ledger, Some(&fin(20)), 1000).is_empty());
    assert_eq!(status_of(&ledger, "a"), Status::Pending);
    assert!(
        ledger.claims.is_empty(),
        "the output must be claimable again on the new branch"
    );
}

#[test]
fn unsettling_from_a_height_reverts_detected_payments_only_from_there() {
    let mut ledger = ledger_with(vec![
        invoice_at("a", ADDR_A, 500, 0, 50),
        invoice_at("b", ADDR_B, 700, 0, 50),
    ]);
    detect(&mut ledger, &block(5, "tx1", ADDR_A, 500));
    detect(&mut ledger, &block(9, "tx2", ADDR_B, 700));
    assert_eq!(roll_back(&mut ledger, 9), 1);
    assert_eq!(status_of(&ledger, "a"), Status::Detected);
    assert_eq!(status_of(&ledger, "b"), Status::Pending);
    assert!(!ledger.claims.contains_key(&claim_key("tx2", 0)));
}

#[test]
fn an_expired_invoice_is_not_confirmed_by_a_late_scan() {
    let mut ledger = ledger_with(vec![invoice("a", 500, 0, 10)]);
    expire(&mut ledger, 10);
    ledger.block_ids.insert(100, "b100".into());
    assert!(confirm(&mut ledger, Some(&fin(100)), 0).is_empty());
    assert_eq!(status_of(&ledger, "a"), Status::Expired);
}

#[test]
fn dust_to_the_merchant_cannot_grow_the_unmatched_records_without_bound() {
    // A flood of unmatched outputs keeps only the newest UNMATCHED_CAP records,
    // and each evicted record releases its claim.
    let mut ledger = ledger_with(vec![invoice("a", 5000, 0, 50)]);
    let txs: Vec<TxOutputs> = (1..=(UNMATCHED_CAP as u64 + 5))
        .map(|amount| TxOutputs {
            txid: format!("dust{amount}"),
            outputs: vec![Output {
                recipient: ADDR_A,
                amount: U256::from(amount),
            }],
        })
        .collect();
    let flood = BlockOutputs {
        height: 5,
        id: "b5".into(),
        prev_id: "b4".into(),
        txs,
    };
    detect(&mut ledger, &flood);
    assert_eq!(ledger.unmatched.len(), UNMATCHED_CAP);
    assert_eq!(
        ledger.unmatched[0].amount,
        U256::from(6u64),
        "the oldest five are evicted"
    );
    assert_eq!(ledger.claims.len(), UNMATCHED_CAP);
}
