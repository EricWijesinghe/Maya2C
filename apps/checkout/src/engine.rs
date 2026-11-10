//! The two loops that move money-facing state: scanning the chain for payments,
//! and delivering signed webhooks.
//!
//! The store lock is held only around in-memory work and the commit, never
//! across a network call, so an RPC or webhook that hangs cannot freeze the
//! HTTP API.

use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::chain::{BlockOutputs, ChainSource, MAX_RANGE};
use crate::error::{CheckoutError, Result};
use crate::invoice::{Delivery, Invoice};
use crate::matching::{confirm, detect, expire, roll_back};
use crate::ssrf::{self, Target};
use crate::store::{RETAIN_BLOCKS, Store};
use crate::webhook::{self, HEADER_NONCE, HEADER_SIGNATURE, HEADER_TIMESTAMP, MAX_ATTEMPTS};

/// Per-request timeout for a webhook, in seconds. A shop that cannot answer in
/// this time gets a retry, not a held connection.
const WEBHOOK_TIMEOUT_SECS: u64 = 10;
/// Longest a destination's DNS lookup may take before the attempt is abandoned.
const DNS_TIMEOUT_SECS: u64 = 5;

/// Sends one HTTP POST to a vetted target. A trait so tests can record the
/// requests instead of making them.
#[async_trait]
pub trait WebhookSender: Send + Sync {
    /// Posts `body` with `headers` and returns the response status.
    async fn post(
        &self,
        target: &Target,
        headers: Vec<(&'static str, String)>,
        body: Vec<u8>,
    ) -> Result<u16>;
}

/// Production sender: HTTPS only, no redirects, connection pinned to the
/// addresses the SSRF check approved.
pub struct HttpSender;

#[async_trait]
impl WebhookSender for HttpSender {
    async fn post(
        &self,
        target: &Target,
        headers: Vec<(&'static str, String)>,
        body: Vec<u8>,
    ) -> Result<u16> {
        let host = target
            .url
            .host_str()
            .ok_or_else(|| CheckoutError::Internal("vetted url has no host".into()))?;
        // A proxy from the environment would resolve the host itself, which
        // skips the address pinning below: refuse proxies outright.
        let client = reqwest::Client::builder()
            .no_proxy()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(WEBHOOK_TIMEOUT_SECS))
            .resolve_to_addrs(host, &target.addrs)
            .build()
            .map_err(|e| CheckoutError::Internal(format!("http client: {}", e.without_url())))?;
        let mut request = client.post(target.url.clone()).body(body);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let response = request
            .send()
            .await
            .map_err(|e| CheckoutError::Internal(format!("webhook send: {}", e.without_url())))?;
        Ok(response.status().as_u16())
    }
}

/// Reads the scan position, which must have been set at startup.
fn scan_position(store: &Store) -> Result<u64> {
    store
        .ledger()
        .scanned_to
        .ok_or_else(|| CheckoutError::Internal("scan position not set at startup".into()))
}

/// Examines every block up to the node's tip, then confirms and expires.
/// Returns the tip height.
///
/// # Errors
///
/// [`CheckoutError::Chain`] when the node cannot answer, or returns blocks out
/// of sequence. Progress made before the error is already committed.
pub async fn scan(
    store: &Mutex<Store>,
    chain: &dyn ChainSource,
    confirmations: u64,
    now_unix: u64,
) -> Result<u64> {
    let tip = chain.tip().await?;
    loop {
        // Re-read the position every batch: a rollback moves it backwards.
        let next = scan_position(&*store.lock().await)? + 1;
        if next > tip.height {
            break;
        }
        let count = (tip.height - next + 1).min(MAX_RANGE);
        let blocks = chain.blocks(next, count).await?;
        if blocks.is_empty() {
            break;
        }
        apply_batch(store, next, &blocks).await?;
    }
    let mut guard = store.lock().await;
    let scanned = scan_position(&guard)?;
    let ledger = guard.ledger_mut();
    confirm(ledger, tip.height, confirmations, now_unix);
    expire(ledger, scanned);
    guard.commit()?;
    Ok(tip.height)
}

/// What one batch did to the ledger.
#[derive(Debug, PartialEq, Eq)]
enum Progress {
    /// The blocks were examined and the scan position advanced.
    Applied,
    /// The batch did not follow the stored chain. The ledger was rolled back
    /// one step and nothing from the batch was examined.
    RolledBack,
}

/// Applies one batch that starts at `first`. Checks the sequence and the
/// internal links before any change, so a malformed batch leaves the ledger
/// untouched. If the first block does not descend from the stored block at
/// `first - 1`, that stored block was reorged out: roll back over it.
async fn apply_batch(store: &Mutex<Store>, first: u64, blocks: &[BlockOutputs]) -> Result<Progress> {
    for (offset, block) in blocks.iter().enumerate() {
        let expected = first.saturating_add(offset as u64);
        if block.height != expected {
            return Err(CheckoutError::Chain(format!(
                "expected block {expected}, node sent {}",
                block.height
            )));
        }
    }
    if blocks.windows(2).any(|pair| pair[1].prev_id != pair[0].id) {
        return Err(CheckoutError::Chain("node sent blocks that do not link".into()));
    }
    let mut guard = store.lock().await;
    // Genesis is never rolled back, so the check starts at height 2.
    let stored_parent = first.checked_sub(1).filter(|h| *h >= 1);
    let stored = stored_parent.and_then(|h| guard.ledger().block_ids.get(&h).cloned());
    if let Some(parent) = stored.filter(|parent| *parent != blocks[0].prev_id) {
        // `stored` is only Some when `stored_parent` is, so `first - 1` is the orphan.
        let orphan = first - 1;
        let reverted = roll_back(guard.ledger_mut(), orphan);
        tracing::warn!(height = orphan, reverted, stored = %parent, "reorg detected; rolled back");
        guard.commit()?;
        return Ok(Progress::RolledBack);
    }
    for block in blocks {
        detect(guard.ledger_mut(), block);
        let ledger = guard.ledger_mut();
        ledger.block_ids.insert(block.height, block.id.clone());
        ledger.scanned_to = Some(block.height);
    }
    if let Some(top) = guard.ledger().scanned_to {
        let keep_from = top.saturating_sub(RETAIN_BLOCKS);
        let ledger = guard.ledger_mut();
        ledger.block_ids = ledger.block_ids.split_off(&keep_from);
    }
    guard.commit()?;
    Ok(Progress::Applied)
}

/// Sends every due webhook once. Failures are recorded and retried later.
/// Returns how many were attempted.
///
/// # Errors
///
/// [`CheckoutError::Store`] if a result cannot be committed.
pub async fn deliver(
    store: &Mutex<Store>,
    sender: &dyn WebhookSender,
    secret: &[u8],
    now_unix: u64,
) -> Result<usize> {
    let jobs = due_jobs(store, now_unix).await?;
    for job in &jobs {
        let delivered = attempt(sender, secret, job, now_unix).await;
        record(store, &job.id, delivered, now_unix).await?;
    }
    Ok(jobs.len())
}

/// One webhook ready to send: the invoice id, its destination and the body.
struct Job {
    id: String,
    url: String,
    body: Vec<u8>,
}

async fn due_jobs(store: &Mutex<Store>, now_unix: u64) -> Result<Vec<Job>> {
    let guard = store.lock().await;
    guard
        .ledger()
        .invoices
        .iter()
        .filter(|inv| inv.delivery == Delivery::Due && inv.next_attempt_unix <= now_unix)
        .map(|inv| -> Result<Job> {
            Ok(Job {
                id: inv.id.clone(),
                url: inv.webhook_url.clone(),
                body: payload(inv)?,
            })
        })
        .collect()
}

/// The JSON a shop receives. `event_id` lets the shop ignore a retry it has
/// already processed.
fn payload(inv: &Invoice) -> Result<Vec<u8>> {
    let payment = inv
        .payment
        .as_ref()
        .ok_or_else(|| CheckoutError::Internal("confirmed invoice has no payment".into()))?;
    let body = serde_json::json!({
        "event_id": format!("{}:confirmed", inv.id),
        "event": "invoice.confirmed",
        "invoice_id": inv.id,
        "memo": inv.memo,
        "amount": inv.amount.to_string(),
        "amount_paid": payment.amount_paid.to_string(),
        "txid": payment.txid,
        "index": payment.index,
        "height": payment.height,
    });
    serde_json::to_vec(&body).map_err(|e| CheckoutError::Internal(format!("encode payload: {e}")))
}

/// One delivery attempt. Every failure is logged and reported as `false`.
async fn attempt(sender: &dyn WebhookSender, secret: &[u8], job: &Job, now_unix: u64) -> bool {
    let vetted = tokio::time::timeout(Duration::from_secs(DNS_TIMEOUT_SECS), ssrf::vet(&job.url)).await;
    let target = match vetted {
        Ok(Ok(target)) => target,
        Ok(Err(reason)) => {
            tracing::warn!(invoice = %job.id, %reason, "webhook destination refused");
            return false;
        }
        Err(_) => {
            tracing::warn!(invoice = %job.id, "webhook destination lookup timed out");
            return false;
        }
    };
    let signed = webhook::new_nonce().and_then(|nonce| {
        let sig = webhook::sign(secret, now_unix, &nonce, &job.body)?;
        Ok((nonce, sig))
    });
    let (nonce, signature) = match signed {
        Ok(pair) => pair,
        Err(e) => {
            tracing::error!(invoice = %job.id, error = %e, "cannot sign webhook");
            return false;
        }
    };
    let headers = vec![
        ("content-type", "application/json".to_owned()),
        (HEADER_TIMESTAMP, now_unix.to_string()),
        (HEADER_NONCE, nonce),
        (HEADER_SIGNATURE, signature),
    ];
    match sender.post(&target, headers, job.body.clone()).await {
        Ok(status) => (200..300).contains(&status),
        Err(e) => {
            tracing::warn!(invoice = %job.id, error = %e, "webhook attempt failed");
            false
        }
    }
}

/// Records the outcome: success ends the job, failure schedules a retry or
/// gives up after [`MAX_ATTEMPTS`].
async fn record(store: &Mutex<Store>, id: &str, delivered: bool, now_unix: u64) -> Result<()> {
    // Randomness is drawn before the lock: the lock is never held across I/O.
    let jitter_seed = webhook::random_u64()?;
    let mut guard = store.lock().await;
    let Some(inv) = guard.ledger_mut().invoices.iter_mut().find(|inv| inv.id == id) else {
        return Ok(());
    };
    if delivered {
        inv.delivery = Delivery::Delivered;
    } else {
        inv.attempts = inv.attempts.saturating_add(1);
        if inv.attempts >= MAX_ATTEMPTS {
            inv.delivery = Delivery::Failed;
        } else {
            let delay = webhook::backoff_jittered_secs(inv.attempts, jitter_seed);
            inv.next_attempt_unix = now_unix.saturating_add(delay);
        }
    }
    guard.commit()
}

/// One full cycle: scan, then deliver.
///
/// # Errors
///
/// Any error from [`scan`] or [`deliver`].
pub async fn run_once(
    store: &Mutex<Store>,
    chain: &dyn ChainSource,
    sender: &dyn WebhookSender,
    secret: &[u8],
    confirmations: u64,
    now_unix: u64,
) -> Result<()> {
    // A failing node must not stop webhooks that are already due: deliver
    // first, then report the scan error.
    let scanned = scan(store, chain, confirmations, now_unix).await;
    deliver(store, sender, secret, now_unix).await?;
    scanned.map(|_| ())
}
