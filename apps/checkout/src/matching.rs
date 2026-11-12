//! Matching chain outputs to invoices. Pure: no I/O, no clock.
//!
//! The rules, each with a test in `tests/payment_matching.rs`:
//! - An output settles an invoice only if it goes to the merchant address, is
//!   exactly the invoice amount, and sits in a block above `created_height` and
//!   at or below `expires_at_height`.
//! - Exactly one pending invoice must fit. Zero or several fit: the output is
//!   recorded as unmatched, never settled by guesswork.
//! - Every other payment to the merchant (over, under, late, early) is recorded
//!   as unmatched for the merchant to refund or assign. It never settles.
//! - One output is claimed once, whatever it matched (the claim is durable).
//! - Settlement is recorded here; confirmation (and the webhook) is decided
//!   by [`confirm`], which re-checks the block id before it counts.

use std::collections::BTreeMap;

use crate::chain::{BlockOutputs, Output, TxOutputs};
use crate::invoice::{Delivery, Invoice, Payment, Status, UnmatchedPayment, UnmatchedReason};
use crate::store::{Claim, Ledger, claim_key};

/// Whether `output` in a block at `height` may settle `invoice`: exact amount,
/// right recipient, pending, inside the window.
fn eligible(invoice: &Invoice, height: u64, output: &Output) -> bool {
    invoice.status == Status::Pending
        && output.recipient == invoice.recipient
        && output.amount == invoice.amount
        && height > invoice.created_height
        && height <= invoice.expires_at_height
}

/// Whether an output goes to the merchant: the configured address, or the
/// address of any invoice the service has issued.
fn is_merchant(ledger: &Ledger, recipient: &[u8; 32]) -> bool {
    ledger.merchant.as_deref() == Some(hex::encode(recipient).as_str())
        || ledger
            .invoices
            .iter()
            .any(|inv| inv.recipient == *recipient)
}

/// Settles pending invoices from one block's outputs. Returns the ids settled.
pub fn detect(ledger: &mut Ledger, block: &BlockOutputs) -> Vec<String> {
    let mut settled = Vec::new();
    for tx in &block.txs {
        for (index, output) in tx.outputs.iter().enumerate() {
            let Ok(index) = u32::try_from(index) else {
                continue;
            };
            let key = claim_key(&tx.txid, index);
            if ledger.claims.contains_key(&key) || !is_merchant(ledger, &output.recipient) {
                continue;
            }
            let fits: Vec<usize> = ledger
                .invoices
                .iter()
                .enumerate()
                .filter(|(_, inv)| eligible(inv, block.height, output))
                .map(|(at, _)| at)
                .collect();
            match fits.as_slice() {
                [only] => {
                    let id = ledger.invoices[*only].id.clone();
                    settle(ledger, *only, &key, tx, index, block, output);
                    settled.push(id);
                }
                [] => record_unmatched(
                    ledger,
                    &key,
                    tx,
                    index,
                    block,
                    output,
                    UnmatchedReason::NoExactInvoice,
                ),
                _ => record_unmatched(
                    ledger,
                    &key,
                    tx,
                    index,
                    block,
                    output,
                    UnmatchedReason::AmbiguousInvoices,
                ),
            }
        }
    }
    settled
}

fn settle(
    ledger: &mut Ledger,
    at: usize,
    key: &str,
    tx: &TxOutputs,
    index: u32,
    block: &BlockOutputs,
    output: &Output,
) {
    let invoice = &mut ledger.invoices[at];
    invoice.status = Status::Detected;
    invoice.payment = Some(Payment {
        txid: tx.txid.clone(),
        index,
        height: block.height,
        block_id: block.id.clone(),
        amount_paid: output.amount,
    });
    ledger
        .claims
        .insert(key.to_owned(), Claim::Invoice(invoice.id.clone()));
}

fn record_unmatched(
    ledger: &mut Ledger,
    key: &str,
    tx: &TxOutputs,
    index: u32,
    block: &BlockOutputs,
    output: &Output,
    reason: UnmatchedReason,
) {
    ledger.unmatched.push(UnmatchedPayment {
        txid: tx.txid.clone(),
        index,
        height: block.height,
        block_id: block.id.clone(),
        recipient: output.recipient,
        amount: output.amount,
        reason,
    });
    ledger.claims.insert(key.to_owned(), Claim::Unmatched);
    while ledger.unmatched.len() > UNMATCHED_CAP {
        let evicted = ledger.unmatched.remove(0);
        ledger
            .claims
            .remove(&claim_key(&evicted.txid, evicted.index));
        tracing::warn!(
            txid = %evicted.txid,
            "unmatched payment records are full; the oldest was evicted"
        );
    }
}

/// Most unmatched records kept. Dust sent to the merchant cannot grow the
/// ledger without bound: the oldest records go first, and each eviction is logged.
pub const UNMATCHED_CAP: usize = 1_000;

/// Confirms detected payments that have `confirmations` blocks on top of them
/// (the payment's own block counts as the first), and queues their webhooks.
///
/// A payment whose block id no longer matches the node's block at that height
/// is not confirmed: it is reverted to pending, so the output can be claimed
/// again on the branch that is now canonical.
pub fn confirm(
    ledger: &mut Ledger,
    tip_height: u64,
    confirmations: u64,
    now_unix: u64,
) -> Vec<String> {
    let depth = confirmations.saturating_sub(1);
    let Ledger {
        invoices,
        claims,
        block_ids,
        ..
    } = ledger;
    let mut confirmed = Vec::new();
    for invoice in invoices.iter_mut() {
        if invoice.status != Status::Detected {
            continue;
        }
        let Some(payment) = invoice.payment.as_ref() else {
            continue;
        };
        let (height, block_id) = (payment.height, payment.block_id.clone());
        if block_ids.get(&height) != Some(&block_id) {
            revert_to_pending(invoice, claims);
            continue;
        }
        if tip_height >= height.saturating_add(depth) {
            invoice.status = Status::Confirmed;
            invoice.delivery = Delivery::Due;
            invoice.next_attempt_unix = now_unix;
            confirmed.push(invoice.id.clone());
        }
    }
    confirmed
}

/// Puts an invoice back to pending and releases its output's claim.
fn revert_to_pending(invoice: &mut Invoice, claims: &mut BTreeMap<String, Claim>) {
    if let Some(payment) = invoice.payment.take() {
        claims.remove(&claim_key(&payment.txid, payment.index));
    }
    invoice.status = Status::Pending;
    invoice.delivery = Delivery::NotDue;
    invoice.attempts = 0;
    invoice.next_attempt_unix = 0;
}

/// Undoes everything seen at `height` and above, after a reorg removed those
/// blocks. Detected and not-yet-sent payments go back to pending; unmatched
/// records are dropped and rediscovered on the new branch. A payment whose
/// webhook was already delivered stays settled and is logged: the shop has
/// acted on it, so this is for the operator to resolve.
///
/// Returns how many invoices were reverted.
pub fn roll_back(ledger: &mut Ledger, height: u64) -> usize {
    let Ledger {
        invoices,
        claims,
        block_ids,
        unmatched,
        scanned_to,
        ..
    } = ledger;
    let mut reverted = 0;
    for invoice in invoices.iter_mut() {
        let Some(payment) = invoice.payment.as_ref() else {
            continue;
        };
        if payment.height < height {
            continue;
        }
        let delivered = invoice.delivery == Delivery::Delivered;
        if delivered {
            invoice.reorged = true;
            tracing::error!(invoice = %invoice.id, height = payment.height, "reorg removed a payment whose webhook was already delivered; flagged for the merchant");
            continue;
        }
        revert_to_pending(invoice, claims);
        reverted += 1;
    }
    unmatched.retain(|u| {
        if u.height < height {
            return true;
        }
        claims.remove(&claim_key(&u.txid, u.index));
        false
    });
    drop(block_ids.split_off(&height));
    *scanned_to = Some(height.saturating_sub(1));
    reverted
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
