//! Matching threats: underpayment, overpayment, double use, timing windows and
//! confirmation depth. Pure logic, no network.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_checkout::chain::{BlockOutputs, Output, TxOutputs};
use maya_checkout::invoice::{Delivery, Invoice, NewInvoice, Status};
use maya_checkout::matching::{confirm, detect, expire};
use maya_checkout::store::{Ledger, claim_key};
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

fn block(height: u64, txid: &str, recipient: [u8; 32], amount: u64) -> BlockOutputs {
    BlockOutputs {
        height,
        txs: vec![TxOutputs {
            txid: txid.into(),
            outputs: vec![Output { recipient, amount: U256::from(amount) }],
        }],
    }
}

fn status_of(ledger: &Ledger, id: &str) -> Status {
    ledger.invoices.iter().find(|i| i.id == id).unwrap().status
}

#[test]
fn underpayment_never_settles() {
    let mut ledger = Ledger { invoices: vec![invoice("a", 1000, 0, 50)], ..Ledger::default() };
    let found = detect(&mut ledger, &block(5, "tx1", MERCHANT, 999));
    assert!(found.is_empty());
    assert_eq!(status_of(&ledger, "a"), Status::Pending);
    assert!(ledger.claims.is_empty(), "an underpayment must not claim an output");
}

#[test]
fn overpayment_settles_and_records_what_arrived() {
    let mut ledger = Ledger { invoices: vec![invoice("a", 1000, 0, 50)], ..Ledger::default() };
    detect(&mut ledger, &block(5, "tx1", MERCHANT, 1500));
    let inv = &ledger.invoices[0];
    assert_eq!(inv.status, Status::Detected);
    assert_eq!(inv.payment.as_ref().unwrap().amount_paid, U256::from(1500u64));
}

#[test]
fn one_output_settles_at_most_one_invoice_even_when_the_block_repeats() {
    // Two open invoices with the same amount (the service refuses this at
    // creation; the matcher is tested on its own so it holds without that check).
    let mut ledger = Ledger {
        invoices: vec![invoice("a", 500, 0, 50), invoice("b", 500, 0, 50)],
        ..Ledger::default()
    };
    let b5 = block(5, "tx1", MERCHANT, 500);
    assert_eq!(detect(&mut ledger, &b5), vec!["a".to_owned()]);
    // The same block seen again (a re-scan after a crash) must not claim again.
    assert!(detect(&mut ledger, &b5).is_empty());
    assert_eq!(status_of(&ledger, "b"), Status::Pending);
    assert_eq!(ledger.claims.get(&claim_key("tx1", 0)).map(String::as_str), Some("a"));
}

#[test]
fn a_claimed_output_cannot_settle_a_later_invoice() {
    let mut ledger = Ledger { invoices: vec![invoice("a", 500, 0, 10)], ..Ledger::default() };
    detect(&mut ledger, &block(5, "tx1", MERCHANT, 500));
    // A new invoice for the same amount, made after block 5, must not be paid by it.
    ledger.invoices.push(invoice("b", 500, 5, 10));
    // Block 6 is above b's creation height, so only the claim can refuse it.
    assert!(detect(&mut ledger, &block(6, "tx1", MERCHANT, 500)).is_empty());
    assert_eq!(status_of(&ledger, "b"), Status::Pending);
}

#[test]
fn payment_in_or_before_the_creation_block_is_ignored() {
    // Created with tip 10: only blocks above 10 can pay. A payment in block 10
    // happened before the invoice was visible.
    let mut ledger = Ledger { invoices: vec![invoice("a", 500, 10, 50)], ..Ledger::default() };
    assert!(detect(&mut ledger, &block(10, "old", MERCHANT, 500)).is_empty());
    assert!(detect(&mut ledger, &block(9, "older", MERCHANT, 500)).is_empty());
    assert_eq!(detect(&mut ledger, &block(11, "new", MERCHANT, 500)), vec!["a".to_owned()]);
}

#[test]
fn payment_after_the_window_is_ignored() {
    let mut ledger = Ledger { invoices: vec![invoice("a", 500, 0, 10)], ..Ledger::default() };
    assert!(detect(&mut ledger, &block(11, "late", MERCHANT, 500)).is_empty());
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
fn payment_to_another_address_is_ignored() {
    let mut ledger = Ledger { invoices: vec![invoice("a", 500, 0, 50)], ..Ledger::default() };
    assert!(detect(&mut ledger, &block(5, "tx1", OTHER, 500)).is_empty());
}

#[test]
fn confirmations_gate_the_webhook() {
    let mut ledger = Ledger { invoices: vec![invoice("a", 500, 0, 50)], ..Ledger::default() };
    detect(&mut ledger, &block(20, "tx1", MERCHANT, 500));
    // Three confirmations: the payment's own block is the first.
    assert!(confirm(&mut ledger, 21, 3, 1000).is_empty());
    assert_eq!(confirm(&mut ledger, 22, 3, 1000), vec!["a".to_owned()]);
    let inv = &ledger.invoices[0];
    assert_eq!(inv.status, Status::Confirmed);
    assert_eq!(inv.delivery, Delivery::Due);
    assert_eq!(inv.next_attempt_unix, 1000);
}

#[test]
fn an_expired_invoice_is_not_confirmed_by_a_late_scan() {
    let mut ledger = Ledger { invoices: vec![invoice("a", 500, 0, 10)], ..Ledger::default() };
    expire(&mut ledger, 10);
    assert!(confirm(&mut ledger, 100, 1, 0).is_empty());
    assert_eq!(status_of(&ledger, "a"), Status::Expired);
}
