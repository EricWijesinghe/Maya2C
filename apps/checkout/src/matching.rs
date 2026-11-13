//! Matching chain outputs to invoices. Pure: no I/O, no clock.
//!
//! The rules, each with a test in `tests/payment_matching.rs`:
//! - Every invoice has its own deposit address, given to no other invoice. An
//!   output is only ever considered for the one invoice its address belongs to.
//! - It settles that invoice only if the invoice is pending, the amount is
//!   exact, and the block is above `created_height` and at or below
//!   `expires_at_height`.
//! - Every other payment to a merchant address (over, under, late, early, a
//!   second payment, an unassigned pool address) is recorded as unmatched,
//!   naming its invoice, for the merchant to refund. It never settles, and it is
//!   never applied to a different invoice.
//! - One output is claimed once, whatever it matched (the claim is durable).
//! - Settlement is recorded here; confirmation (and the webhook) waits for
//!   DAG-BFT finality in [`confirm`], which re-checks the block id first.

use std::collections::BTreeMap;

use crate::chain::{BlockOutputs, Finalized, Output, TxOutputs};
use crate::invoice::{Delivery, Invoice, Payment, Status, UnmatchedPayment, UnmatchedReason};
use crate::store::{Claim, Ledger, claim_key};

/// Whether `output` in a block at `height` settles `invoice`, or why not. The
/// caller has already matched the output's address to this invoice.
fn classify(
    invoice: &Invoice,
    height: u64,
    output: &Output,
) -> std::result::Result<(), UnmatchedReason> {
    if invoice.status != Status::Pending {
        return Err(UnmatchedReason::AlreadySettled);
    }
    if height <= invoice.created_height {
        return Err(UnmatchedReason::Early);
    }
    if height > invoice.expires_at_height {
        return Err(UnmatchedReason::Late);
    }
    match output.amount.cmp(&invoice.amount) {
        std::cmp::Ordering::Less => Err(UnmatchedReason::Underpaid),
        std::cmp::Ordering::Greater => Err(UnmatchedReason::Overpaid),
        std::cmp::Ordering::Equal => Ok(()),
    }
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
            if ledger.claims.contains_key(&key) {
                continue;
            }
            // Addresses are unique per invoice (Store::insert), so at most one.
            let owner = ledger
                .invoices
                .iter()
                .position(|inv| inv.recipient == output.recipient);
            let found = Found {
                key: &key,
                tx,
                index,
                block,
                output,
            };
            match owner {
                Some(at) => match classify(&ledger.invoices[at], block.height, output) {
                    Ok(()) => {
                        settled.push(ledger.invoices[at].id.clone());
                        settle(ledger, at, &found);
                    }
                    Err(reason) => {
                        let id = ledger.invoices[at].id.clone();
                        record_unmatched(ledger, &found, reason, Some(id));
                    }
                },
                None if ledger.addresses.contains(&hex::encode(output.recipient)) => {
                    record_unmatched(ledger, &found, UnmatchedReason::Unassigned, None);
                }
                None => {}
            }
        }
    }
    settled
}

/// One output, where it was found.
struct Found<'a> {
    key: &'a str,
    tx: &'a TxOutputs,
    index: u32,
    block: &'a BlockOutputs,
    output: &'a Output,
}

fn settle(ledger: &mut Ledger, at: usize, found: &Found<'_>) {
    let invoice = &mut ledger.invoices[at];
    invoice.status = Status::Detected;
    invoice.payment = Some(Payment {
        txid: found.tx.txid.clone(),
        index: found.index,
        height: found.block.height,
        block_id: found.block.id.clone(),
        amount_paid: found.output.amount,
    });
    ledger
        .claims
        .insert(found.key.to_owned(), Claim::Invoice(invoice.id.clone()));
}

fn record_unmatched(
    ledger: &mut Ledger,
    found: &Found<'_>,
    reason: UnmatchedReason,
    invoice_id: Option<String>,
) {
    ledger.unmatched.push(UnmatchedPayment {
        invoice_id,
        txid: found.tx.txid.clone(),
        index: found.index,
        height: found.block.height,
        block_id: found.block.id.clone(),
        recipient: found.output.recipient,
        amount: found.output.amount,
        reason,
    });
    ledger.claims.insert(found.key.to_owned(), Claim::Unmatched);
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

/// Confirms detected payments that DAG-BFT has finalized, and queues their
/// webhooks. A payment is final when its block is at or below the newest
/// checkpoint, and the checkpoint's block is the one this service scanned at
/// that height (so the checkpoint and the payment are on one branch). With no
/// checkpoint, or one that cannot be checked, nothing confirms.
///
/// A payment whose block id no longer matches the node's block at that height
/// is reverted to pending, so the output can be claimed again on the branch
/// that is now canonical.
pub fn confirm(ledger: &mut Ledger, finalized: Option<&Finalized>, now_unix: u64) -> Vec<String> {
    let Ledger {
        invoices,
        claims,
        block_ids,
        ..
    } = ledger;
    let final_height = finalized
        .filter(|f| block_ids.get(&f.height) == Some(&f.block_id))
        .map(|f| f.height);
    if let (Some(f), None) = (finalized, final_height) {
        tracing::info!(
            height = f.height,
            "checkpoint is not on the scanned chain yet; nothing confirms this cycle"
        );
    }
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
        if final_height.is_some_and(|f| height <= f) {
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
/// acted on it, so this is for the operator to resolve. (With confirmation at
/// finality this means a finalized block was replaced, which BFT rules out.)
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
