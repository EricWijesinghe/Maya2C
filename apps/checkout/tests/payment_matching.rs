//! Matching threats: exact-amount binding, non-exact payments, double use,
//! timing windows and confirmation depth. Pure logic, no network.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_checkout::chain::{BlockOutputs, Output, TxOutputs};
use maya_checkout::invoice::{Delivery, Invoice, NewInvoice, Status, UnmatchedReason};
use maya_checkout::matching::{UNMATCHED_CAP, confirm, detect, expire, roll_back};
use maya_checkout::store::{Claim, Ledger, claim_key};
use primitive_types::U256;

const MERCHANT: [u8; 32] = [9; 32];
const OTHER: [u8; 32] = [4; 32];

fn invoice(id: &str, amount: u64, created: u64, expires_in: u64) -> Invoice {
    let req = NewInvoice {
        amount: U256::from(amount),
        memo: id.into(),
        expires_in_blocks: expires_in,
        webhook_url: "https://93.184.216.34/hook".into(),
    };
    Invoice::open(id.into(), MERCHANT, &req, created).unwrap()
}

/// A block whose id is `b<height>` and whose parent is `b<height-1>`.
fn block(height: u64, txid: &str, recipient: [u8; 32], amount: u64) -> BlockOutputs {
    BlockOutputs {
        height,
        id: format!("b{height}"),
        prev_id: format!("b{}", height.saturating_sub(1)),
        txs: vec![TxOutputs {
            txid: txid.into(),
            outputs: vec![Output { recipient, amount: U256::from(amount) }],
        }],
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
fn first_pending_invoice_cannot_take_another_invoices_payment() {
    // The reviewed scenario: a 1-unit invoice that will never be paid sits
    // first in creation order. A 1000 payment must settle the 1000 invoice.
    let mut ledger = Ledger {
        invoices: vec![invoice("tiny", 1, 0, 50), invoice("big", 1000, 0, 50)],
        ..Ledger::default()
    };
    assert_eq!(detect(&mut ledger, &block(5, "tx1", MERCHANT, 1000)), vec!["big".to_owned()]);
    assert_eq!(status_of(&ledger, "tiny"), Status::Pending);
    assert_eq!(status_of(&ledger, "big"), Status::Detected);
}

#[test]
fn underpayment_is_recorded_as_unmatched_and_settles_nothing() {
    let mut ledger = Ledger { invoices: vec![invoice("a", 1000, 0, 50)], ..Ledger::default() };
    assert!(detect(&mut ledger, &block(5, "tx1", MERCHANT, 999)).is_empty());
    assert_eq!(status_of(&ledger, "a"), Status::Pending);
    assert_eq!(ledger.unmatched.len(), 1);
    assert_eq!(ledger.unmatched[0].amount, U256::from(999u64));
    assert_eq!(ledger.unmatched[0].reason, UnmatchedReason::NoExactInvoice);
    assert_eq!(ledger.claims.get(&claim_key("tx1", 0)), Some(&Claim::Unmatched));
}

#[test]
fn overpayment_is_recorded_as_unmatched_and_never_settles() {
    let mut ledger = Ledger { invoices: vec![invoice("a", 1000, 0, 50)], ..Ledger::default() };
    assert!(detect(&mut ledger, &block(5, "tx1", MERCHANT, 1500)).is_empty());
    assert_eq!(status_of(&ledger, "a"), Status::Pending);
    assert_eq!(ledger.unmatched.len(), 1);
    assert_eq!(ledger.unmatched[0].amount, U256::from(1500u64));
}

#[test]
fn two_pending_invoices_with_one_amount_settle_nothing() {
    // The service refuses this at creation; if it ever happened anyway, a
    // payment would fit both, so neither may be settled by guesswork.
    let mut ledger = Ledger {
        invoices: vec![invoice("a", 500, 0, 50), invoice("b", 500, 0, 50)],
        ..Ledger::default()
    };
    assert!(detect(&mut ledger, &block(5, "tx1", MERCHANT, 500)).is_empty());
    assert_eq!(status_of(&ledger, "a"), Status::Pending);
    assert_eq!(status_of(&ledger, "b"), Status::Pending);
    assert_eq!(ledger.unmatched[0].reason, UnmatchedReason::AmbiguousInvoices);
}

#[test]
fn one_output_is_claimed_once_even_when_the_block_repeats() {
    let mut ledger = Ledger { invoices: vec![invoice("a", 500, 0, 50)], ..Ledger::default() };
    let b5 = block(5, "tx1", MERCHANT, 500);
    assert_eq!(detect(&mut ledger, &b5), vec!["a".to_owned()]);
    // The same block seen again (a re-scan after a crash) must not claim again.
    assert!(detect(&mut ledger, &b5).is_empty());
    assert_eq!(settled_by(&ledger, "tx1").as_deref(), Some("a"));
    assert_eq!(ledger.unmatched.len(), 0);
}

#[test]
fn a_claimed_output_cannot_settle_a_later_invoice() {
    let mut ledger = Ledger { invoices: vec![invoice("a", 500, 0, 10)], ..Ledger::default() };
    detect(&mut ledger, &block(5, "tx1", MERCHANT, 500));
    // A new invoice for the same amount, made after block 5, must not be paid by it.
    ledger.invoices.push(invoice("b", 500, 5, 10));
    assert!(detect(&mut ledger, &block(6, "tx1", MERCHANT, 500)).is_empty());
    assert_eq!(status_of(&ledger, "b"), Status::Pending);
}

#[test]
fn payment_in_or_before_the_creation_block_is_not_settled() {
    // Created with tip 10: only blocks above 10 can pay. A payment in block 10
    // happened before the invoice was visible.
    let mut ledger = Ledger { invoices: vec![invoice("a", 500, 10, 50)], ..Ledger::default() };
    assert!(detect(&mut ledger, &block(10, "old", MERCHANT, 500)).is_empty());
    assert!(detect(&mut ledger, &block(9, "older", MERCHANT, 500)).is_empty());
    assert_eq!(detect(&mut ledger, &block(11, "new", MERCHANT, 500)), vec!["a".to_owned()]);
}

#[test]
fn payment_after_the_window_is_not_settled() {
    let mut ledger = Ledger { invoices: vec![invoice("a", 500, 0, 10)], ..Ledger::default() };
    assert!(detect(&mut ledger, &block(11, "late", MERCHANT, 500)).is_empty());
    assert_eq!(ledger.unmatched.len(), 1);
    assert_eq!(detect(&mut ledger, &block(10, "last", MERCHANT, 500)), vec!["a".to_owned()]);
}

#[test]
fn expiry_happens_only_after_the_window_is_scanned() {
    let mut ledger = Ledger { invoices: vec![invoice("a", 500, 0, 10)], ..Ledger::default() };
    assert!(expire(&mut ledger, 9).is_empty());
    assert_eq!(expire(&mut ledger, 10), vec!["a".to_owned()]);
    assert_eq!(status_of(&ledger, "a"), Status::Expired);
}

#[test]
fn payment_to_another_address_is_ignored_not_recorded() {
    let mut ledger = Ledger { invoices: vec![invoice("a", 500, 0, 50)], ..Ledger::default() };
    assert!(detect(&mut ledger, &block(5, "tx1", OTHER, 500)).is_empty());
    assert!(ledger.unmatched.is_empty());
    assert!(ledger.claims.is_empty());
}

#[test]
fn confirmations_gate_the_webhook() {
    let mut ledger = Ledger { invoices: vec![invoice("a", 500, 0, 50)], ..Ledger::default() };
    detect(&mut ledger, &block(20, "tx1", MERCHANT, 500));
    ledger.block_ids.insert(20, "b20".into());
    // Three confirmations: the payment's own block is the first.
    assert!(confirm(&mut ledger, 21, 3, 1000).is_empty());
    assert_eq!(confirm(&mut ledger, 22, 3, 1000), vec!["a".to_owned()]);
    let inv = &ledger.invoices[0];
    assert_eq!(inv.status, Status::Confirmed);
    assert_eq!(inv.delivery, Delivery::Due);
    assert_eq!(inv.next_attempt_unix, 1000);
}

#[test]
fn confirm_refuses_a_payment_whose_block_id_changed() {
    let mut ledger = Ledger { invoices: vec![invoice("a", 500, 0, 50)], ..Ledger::default() };
    detect(&mut ledger, &block(20, "tx1", MERCHANT, 500));
    // The node now has a different block at height 20: the payment is not final.
    ledger.block_ids.insert(20, "forked".into());
    assert!(confirm(&mut ledger, 99, 3, 1000).is_empty());
    assert_eq!(status_of(&ledger, "a"), Status::Pending, "the payment must be un-settled");
    assert!(ledger.claims.is_empty(), "the output must be claimable again on the new branch");
}

#[test]
fn unsettling_from_a_height_reverts_detected_payments_only_from_there() {
    let mut ledger = Ledger {
        invoices: vec![invoice("a", 500, 0, 50), invoice("b", 700, 0, 50)],
        ..Ledger::default()
    };
    detect(&mut ledger, &block(5, "tx1", MERCHANT, 500));
    detect(&mut ledger, &block(9, "tx2", MERCHANT, 700));
    assert_eq!(roll_back(&mut ledger, 9), 1);
    assert_eq!(status_of(&ledger, "a"), Status::Detected);
    assert_eq!(status_of(&ledger, "b"), Status::Pending);
    assert!(!ledger.claims.contains_key(&claim_key("tx2", 0)));
}

#[test]
fn an_expired_invoice_is_not_confirmed_by_a_late_scan() {
    let mut ledger = Ledger { invoices: vec![invoice("a", 500, 0, 10)], ..Ledger::default() };
    expire(&mut ledger, 10);
    assert!(confirm(&mut ledger, 100, 1, 0).is_empty());
    assert_eq!(status_of(&ledger, "a"), Status::Expired);
}

#[test]
fn dust_to_the_merchant_cannot_grow_the_unmatched_records_without_bound() {
    // N3: a flood of unmatched outputs keeps only the newest UNMATCHED_CAP records,
    // and each evicted record releases its claim.
    let mut ledger = Ledger { invoices: vec![invoice("a", 5000, 0, 50)], ..Ledger::default() };
    let txs: Vec<TxOutputs> = (1..=(UNMATCHED_CAP as u64 + 5))
        .map(|amount| TxOutputs {
            txid: format!("dust{amount}"),
            outputs: vec![Output { recipient: MERCHANT, amount: U256::from(amount) }],
        })
        .collect();
    let flood = BlockOutputs { height: 5, id: "b5".into(), prev_id: "b4".into(), txs };
    detect(&mut ledger, &flood);
    assert_eq!(ledger.unmatched.len(), UNMATCHED_CAP);
    assert_eq!(ledger.unmatched[0].amount, U256::from(6u64), "the oldest five are evicted");
    assert_eq!(ledger.claims.len(), UNMATCHED_CAP);
}
