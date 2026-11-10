//! Matching chain outputs to invoices. Pure: no I/O, no clock.
//!
//! The rules, each with a test in `tests/payment_matching.rs`:
//! - An output pays an invoice only if it goes to the invoice's recipient, is
//!   at least the amount due, and sits in a block above `created_height` and at
//!   or below `expires_at_height`.
//! - Underpayment never pays. Overpayment pays in full and records what
//!   arrived.
//! - One output settles at most one invoice (the claim is durable).
//! - Matching walks invoices in creation order, so the outcome is deterministic.

use crate::chain::{BlockOutputs, Output};
use crate::invoice::{Delivery, Invoice, Payment, Status};
use crate::store::{claim_key, Ledger};

/// Whether `output` in a block at `height` may settle `invoice`.
fn eligible(invoice: &Invoice, height: u64, output: &Output) -> bool {
    invoice.status == Status::Pending
        && output.recipient == invoice.recipient
        && output.amount >= invoice.amount
        && height > invoice.created_height
        && height <= invoice.expires_at_height
}

/// Settles pending invoices from one block's outputs. Returns the ids detected.
pub fn detect(ledger: &mut Ledger, block: &BlockOutputs) -> Vec<String> {
    let mut detected = Vec::new();
    for tx in &block.txs {
        for (index, output) in tx.outputs.iter().enumerate() {
            let Ok(index) = u32::try_from(index) else { continue };
            let key = claim_key(&tx.txid, index);
            if ledger.claims.contains_key(&key) {
                continue;
            }
            let Some(invoice) = ledger
                .invoices
                .iter_mut()
                .find(|inv| eligible(inv, block.height, output))
            else {
                continue;
            };
            invoice.status = Status::Detected;
            invoice.payment = Some(Payment {
                txid: tx.txid.clone(),
                index,
                height: block.height,
                amount_paid: output.amount,
            });
            ledger.claims.insert(key, invoice.id.clone());
            detected.push(invoice.id.clone());
        }
    }
    detected
}

/// Confirms detected payments that have `confirmations` blocks on top of them
/// (the payment's own block counts as the first), and queues their webhooks.
pub fn confirm(ledger: &mut Ledger, tip_height: u64, confirmations: u64, now_unix: u64) -> Vec<String> {
    let depth = confirmations.saturating_sub(1);
    let mut confirmed = Vec::new();
    for invoice in &mut ledger.invoices {
        let Some(payment) = &invoice.payment else { continue };
        if invoice.status != Status::Detected {
            continue;
        }
        if tip_height >= payment.height.saturating_add(depth) {
            invoice.status = Status::Confirmed;
            invoice.delivery = Delivery::Due;
            invoice.next_attempt_unix = now_unix;
            confirmed.push(invoice.id.clone());
        }
    }
    confirmed
}

/// Expires pending invoices whose window has fully been scanned without a
/// payment. `scanned_to` is the highest height whose outputs were examined.
pub fn expire(ledger: &mut Ledger, scanned_to: u64) -> Vec<String> {
    let mut expired = Vec::new();
    for invoice in &mut ledger.invoices {
        if invoice.status == Status::Pending && invoice.expires_at_height <= scanned_to {
            invoice.status = Status::Expired;
            expired.push(invoice.id.clone());
        }
    }
    expired
}
