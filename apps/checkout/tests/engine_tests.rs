//! The whole path, offline: an in-memory chain, a recording webhook sender, and
//! a real file-backed ledger.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::sync::Mutex as StdMutex;

use async_trait::async_trait;
use maya_checkout::chain::{BlockOutputs, ChainSource, Output, Tip, TxOutputs};
use maya_checkout::engine::{WebhookSender, deliver, run_once, scan};
use maya_checkout::invoice::{Delivery, Invoice, NewInvoice, Status};
use maya_checkout::ssrf::Target;
use maya_checkout::store::Store;
use maya_checkout::webhook::{
    HEADER_NONCE, HEADER_SIGNATURE, HEADER_TIMESTAMP, MAX_ATTEMPTS, NonceCache, backoff_secs, verify,
};
use maya_checkout::{CheckoutError, Result};
use primitive_types::U256;
use tokio::sync::Mutex;

const MERCHANT: [u8; 32] = [9; 32];
const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";
const NOW: u64 = 1_800_000_000;
const PUBLIC_HOOK: &str = "https://93.184.216.34/hook";

struct FakeChain {
    height: u64,
    blocks: Vec<BlockOutputs>,
}

#[async_trait]
impl ChainSource for FakeChain {
    async fn tip(&self) -> Result<Tip> {
        Ok(Tip { height: self.height, chain_id: Some("maya-testnet-1".into()) })
    }
    async fn blocks(&self, from: u64, count: u64) -> Result<Vec<BlockOutputs>> {
        Ok(self
            .blocks
            .iter()
            .filter(|b| b.height >= from && b.height < from + count)
            .cloned()
            .collect())
    }
}

/// Each recorded delivery: its headers and body.
type Sent = Vec<(Vec<(&'static str, String)>, Vec<u8>)>;

#[derive(Default)]
struct RecordingSender {
    status: u16,
    sent: StdMutex<Sent>,
}

#[async_trait]
impl WebhookSender for RecordingSender {
    async fn post(
        &self,
        _target: &Target,
        headers: Vec<(&'static str, String)>,
        body: Vec<u8>,
    ) -> Result<u16> {
        self.sent.lock().unwrap().push((headers, body));
        Ok(self.status)
    }
}

fn temp_ledger(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("maya-checkout-engine-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let _ = std::fs::remove_file(&path);
    path
}

/// Block ids: heights below `fork_at` are on the shared branch `a`; the rest
/// are on `tag`. A branch with `fork_at` = `u64::MAX` is simply branch `a`.
fn id_of(height: u64, fork_at: u64, tag: &str) -> String {
    if height < fork_at { format!("a{height}") } else { format!("{tag}{height}") }
}

/// A chain up to `tip` with an optional `(height, amount)` payment to the merchant.
fn branch(tip: u64, fork_at: u64, tag: &str, payment: Option<(u64, u64)>) -> FakeChain {
    let blocks = (1..=tip)
        .map(|h| {
            let txs = match payment {
                Some((pay_height, amount)) if pay_height == h => vec![TxOutputs {
                    txid: "pay".into(),
                    outputs: vec![Output { recipient: MERCHANT, amount: U256::from(amount) }],
                }],
                _ => vec![],
            };
            BlockOutputs {
                height: h,
                id: id_of(h, fork_at, tag),
                prev_id: id_of(h - 1, fork_at, tag),
                txs,
            }
        })
        .collect();
    FakeChain { height: tip, blocks }
}

fn chain_with_payment(tip: u64, pay_height: u64, amount: u64) -> FakeChain {
    branch(tip, u64::MAX, "a", Some((pay_height, amount)))
}

/// A node that cannot answer at all.
struct DownChain;

#[async_trait]
impl ChainSource for DownChain {
    async fn tip(&self) -> Result<Tip> {
        Err(CheckoutError::Chain("node down".into()))
    }
    async fn blocks(&self, _from: u64, _count: u64) -> Result<Vec<BlockOutputs>> {
        Err(CheckoutError::Chain("node down".into()))
    }
}

fn open_invoice(store: &mut Store, id: &str, amount: u64, tip_at_creation: u64) {
    let req = NewInvoice {
        amount: U256::from(amount),
        memo: "order".into(),
        expires_in_blocks: 100,
        webhook_url: PUBLIC_HOOK.into(),
    };
    let inv = Invoice::open(id.into(), MERCHANT, &req, tip_at_creation).unwrap();
    store.insert(inv).unwrap();
}

#[tokio::test]
async fn payment_is_detected_confirmed_and_delivered_with_a_verifiable_signature() {
    let path = temp_ledger("happy.json");
    let mut store = Store::open(&path).unwrap();
    store.ledger_mut().scanned_to = Some(0);
    open_invoice(&mut store, "inv1", 500, 0);
    let store = Mutex::new(store);
    let chain = chain_with_payment(12, 5, 500);
    let sender = RecordingSender { status: 200, ..Default::default() };

    run_once(&store, &chain, &sender, SECRET, 3, NOW).await.unwrap();

    let guard = store.lock().await;
    let inv = guard.find("inv1").unwrap();
    assert_eq!(inv.status, Status::Confirmed);
    assert_eq!(inv.delivery, Delivery::Delivered);
    drop(guard);

    let sent = sender.sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    let (headers, body) = &sent[0];
    let header = |name: &str| headers.iter().find(|(k, _)| *k == name).unwrap().1.clone();
    let nonce = header(HEADER_NONCE);
    let timestamp: u64 = header(HEADER_TIMESTAMP).parse().unwrap();
    assert_eq!(verify(SECRET, timestamp, &nonce, body, &header(HEADER_SIGNATURE), NOW), Ok(()));
    let json: serde_json::Value = serde_json::from_slice(body).unwrap();
    assert_eq!(json["event_id"], "inv1:confirmed");
    assert_eq!(json["amount_paid"], "500");
    assert!(json.get("overpaid").is_none(), "only exact payments settle, so no overpaid field");
}

#[tokio::test]
async fn a_payment_below_the_confirmation_depth_is_not_delivered_yet() {
    let path = temp_ledger("depth.json");
    let mut store = Store::open(&path).unwrap();
    store.ledger_mut().scanned_to = Some(0);
    open_invoice(&mut store, "inv1", 500, 0);
    let store = Mutex::new(store);
    // Payment in block 5, tip 6, depth 3: needs tip >= 7.
    let chain = chain_with_payment(6, 5, 500);
    let sender = RecordingSender { status: 200, ..Default::default() };

    run_once(&store, &chain, &sender, SECRET, 3, NOW).await.unwrap();

    assert_eq!(store.lock().await.find("inv1").unwrap().status, Status::Detected);
    assert!(sender.sent.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_failing_shop_is_retried_on_the_backoff_schedule_then_marked_failed() {
    let path = temp_ledger("retry.json");
    let mut store = Store::open(&path).unwrap();
    store.ledger_mut().scanned_to = Some(0);
    open_invoice(&mut store, "inv1", 500, 0);
    let store = Mutex::new(store);
    let chain = chain_with_payment(10, 2, 500);
    let sender = RecordingSender { status: 500, ..Default::default() };

    run_once(&store, &chain, &sender, SECRET, 1, NOW).await.unwrap();
    {
        let guard = store.lock().await;
        let inv = guard.find("inv1").unwrap();
        assert_eq!(inv.attempts, 1);
        let base = NOW + backoff_secs(1);
        assert!(inv.next_attempt_unix >= base && inv.next_attempt_unix <= base + backoff_secs(1) / 4);
        assert_eq!(inv.delivery, Delivery::Due);
    }

    // Not yet due: no new attempt.
    deliver(&store, &sender, SECRET, NOW + 1).await.unwrap();
    assert_eq!(sender.sent.lock().unwrap().len(), 1);

    for _ in 1..MAX_ATTEMPTS {
        let due_at = store.lock().await.find("inv1").unwrap().next_attempt_unix;
        deliver(&store, &sender, SECRET, due_at).await.unwrap();
    }
    let guard = store.lock().await;
    let inv = guard.find("inv1").unwrap();
    assert_eq!(inv.delivery, Delivery::Failed);
    assert_eq!(inv.attempts, MAX_ATTEMPTS);
    assert_eq!(sender.sent.lock().unwrap().len(), MAX_ATTEMPTS as usize);
}

#[tokio::test]
async fn each_delivery_gets_a_fresh_nonce_so_a_replay_is_detectable() {
    let path = temp_ledger("nonce.json");
    let mut store = Store::open(&path).unwrap();
    store.ledger_mut().scanned_to = Some(0);
    open_invoice(&mut store, "inv1", 500, 0);
    let store = Mutex::new(store);
    let chain = chain_with_payment(10, 2, 500);
    let sender = RecordingSender { status: 500, ..Default::default() };
    run_once(&store, &chain, &sender, SECRET, 1, NOW).await.unwrap();
    deliver(&store, &sender, SECRET, NOW + 2 * backoff_secs(1)).await.unwrap();

    let sent = sender.sent.lock().unwrap();
    let nonce = |i: usize| sent[i].0.iter().find(|(k, _)| *k == HEADER_NONCE).unwrap().1.clone();
    assert_ne!(nonce(0), nonce(1));
    let mut receiver = NonceCache::default();
    assert!(receiver.accept(&nonce(0), NOW, NOW).is_ok());
    assert!(receiver.accept(&nonce(0), NOW, NOW).is_err());
}

#[tokio::test]
async fn a_block_out_of_sequence_changes_nothing() {
    let path = temp_ledger("sequence.json");
    let mut store = Store::open(&path).unwrap();
    store.ledger_mut().scanned_to = Some(0);
    open_invoice(&mut store, "inv1", 500, 0);
    let store = Mutex::new(store);
    let mut chain = chain_with_payment(4, 2, 500);
    chain.blocks[1].height = 9; // node skips height 2 and says it is 9
    let result = scan(&store, &chain, 1, NOW).await;
    assert!(matches!(result, Err(CheckoutError::Chain(_))));
    let guard = store.lock().await;
    assert_eq!(guard.ledger().scanned_to, Some(0));
    assert_eq!(guard.find("inv1").unwrap().status, Status::Pending);
}

#[tokio::test]
async fn the_scan_position_survives_a_restart() {
    let path = temp_ledger("restart.json");
    let mut store = Store::open(&path).unwrap();
    store.ledger_mut().scanned_to = Some(0);
    open_invoice(&mut store, "inv1", 500, 0);
    let store_mutex = Mutex::new(store);
    let chain = chain_with_payment(8, 3, 500);
    scan(&store_mutex, &chain, 1, NOW).await.unwrap();
    drop(store_mutex);

    let reopened = Store::open(&path).unwrap();
    assert_eq!(reopened.ledger().scanned_to, Some(8));
    // One confirmation: block 3 with tip 8 is already confirmed.
    assert_eq!(reopened.find("inv1").unwrap().status, Status::Confirmed);
    assert_eq!(reopened.ledger().claims.len(), 1);
}

#[tokio::test]
async fn a_private_webhook_destination_is_never_contacted() {
    let path = temp_ledger("refuse.json");
    let mut store = Store::open(&path).unwrap();
    store.ledger_mut().scanned_to = Some(0);
    let req = NewInvoice {
        amount: U256::from(500u64),
        memo: "x".into(),
        expires_in_blocks: 100,
        webhook_url: "https://10.0.0.7/hook".into(),
    };
    // Stored directly: the HTTP path would refuse this URL; the engine must too.
    store.ledger_mut().invoices.push(Invoice::open("inv1".into(), MERCHANT, &req, 0).unwrap());
    let store = Mutex::new(store);
    let chain = chain_with_payment(10, 2, 500);
    let sender = RecordingSender { status: 200, ..Default::default() };
    run_once(&store, &chain, &sender, SECRET, 1, NOW).await.unwrap();
    assert!(sender.sent.lock().unwrap().is_empty(), "a private destination was contacted");
    assert_eq!(store.lock().await.find("inv1").unwrap().attempts, 1);
}

/// The reorg scenario: the payment sits in block 5 of branch `a`. Branch `b`
/// replaces everything from height 4 up, and has no payment.
#[tokio::test]
async fn a_reorged_payment_is_unsettled_and_never_confirmed() {
    let path = temp_ledger("reorg-away.json");
    let mut store = Store::open(&path).unwrap();
    store.ledger_mut().scanned_to = Some(0);
    open_invoice(&mut store, "inv1", 500, 0);
    let store = Mutex::new(store);

    // Confirmation depth 10: a payment at 5 with tip 12 is detected, not final.
    scan(&store, &branch(12, u64::MAX, "a", Some((5, 500))), 10, NOW).await.unwrap();
    {
        let guard = store.lock().await;
        let inv = guard.find("inv1").unwrap();
        assert_eq!(inv.status, Status::Detected);
        assert_eq!(inv.payment.as_ref().unwrap().block_id, "a5");
    }

    scan(&store, &branch(14, 4, "b", None), 10, NOW).await.unwrap();

    let guard = store.lock().await;
    assert_eq!(guard.find("inv1").unwrap().status, Status::Pending);
    assert!(guard.find("inv1").unwrap().payment.is_none());
    assert!(guard.ledger().claims.is_empty(), "the orphaned output must be claimable again");
    assert_eq!(guard.ledger().block_ids.get(&5).map(String::as_str), Some("b5"));
}

/// The same reorg, but the replacement branch carries the payment in block 6.
/// It is settled from that branch, with that branch's block id.
#[tokio::test]
async fn a_payment_on_the_replacement_branch_is_detected_from_that_branch() {
    let path = temp_ledger("reorg-again.json");
    let mut store = Store::open(&path).unwrap();
    store.ledger_mut().scanned_to = Some(0);
    open_invoice(&mut store, "inv1", 500, 0);
    let store = Mutex::new(store);

    scan(&store, &branch(12, u64::MAX, "a", Some((5, 500))), 10, NOW).await.unwrap();
    scan(&store, &branch(14, 4, "b", Some((6, 500))), 10, NOW).await.unwrap();

    let guard = store.lock().await;
    let inv = guard.find("inv1").unwrap();
    assert_eq!(inv.status, Status::Detected);
    assert_eq!(inv.payment.as_ref().unwrap().height, 6);
    assert_eq!(inv.payment.as_ref().unwrap().block_id, "b6");
}

/// A dead node must not stop webhook retries for invoices already confirmed.
#[tokio::test]
async fn a_failed_scan_does_not_stop_webhook_delivery() {
    let path = temp_ledger("scan-down.json");
    let mut store = Store::open(&path).unwrap();
    store.ledger_mut().scanned_to = Some(0);
    open_invoice(&mut store, "inv1", 500, 0);
    let store = Mutex::new(store);
    // The shop is down, so the first attempt fails and a retry is scheduled.
    let down_shop = RecordingSender { status: 500, ..Default::default() };
    run_once(&store, &chain_with_payment(10, 2, 500), &down_shop, SECRET, 1, NOW).await.unwrap();
    assert_eq!(down_shop.sent.lock().unwrap().len(), 1);
    let retry_at = store.lock().await.find("inv1").unwrap().next_attempt_unix;

    // Later the node is gone but the shop is back: the retry must still go out.
    let back_up = RecordingSender { status: 200, ..Default::default() };
    let result = run_once(&store, &DownChain, &back_up, SECRET, 1, retry_at).await;

    assert!(matches!(result, Err(CheckoutError::Chain(_))), "the scan error is still reported");
    assert_eq!(back_up.sent.lock().unwrap().len(), 1, "the due retry was not sent");
}
