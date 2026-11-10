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

#[derive(Default)]
struct RecordingSender {
    status: u16,
    sent: StdMutex<Vec<(Vec<(&'static str, String)>, Vec<u8>)>>,
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

fn payment_block(height: u64, amount: u64) -> BlockOutputs {
    BlockOutputs {
        height,
        txs: vec![TxOutputs {
            txid: "pay".into(),
            outputs: vec![Output { recipient: MERCHANT, amount: U256::from(amount) }],
        }],
    }
}

fn chain_with_payment(tip: u64, pay_height: u64, amount: u64) -> FakeChain {
    let blocks = (1..=tip)
        .map(|h| if h == pay_height { payment_block(h, amount) } else { BlockOutputs { height: h, txs: vec![] } })
        .collect();
    FakeChain { height: tip, blocks }
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
    assert_eq!(json["overpaid"], false);
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
        assert_eq!(inv.next_attempt_unix, NOW + backoff_secs(1));
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
    deliver(&store, &sender, SECRET, NOW + backoff_secs(1)).await.unwrap();

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
