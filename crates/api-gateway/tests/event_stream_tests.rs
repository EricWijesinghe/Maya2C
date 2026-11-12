//! The event feed: blocks and transactions read from the node, filtered by
//! address or asset, and streamed over a WebSocket.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use maya_api_gateway::error::GatewayError;
use maya_api_gateway::events::{self, Event, Events, EventsConfig, Filter};
use maya_api_gateway::node::{Balance, ChainInfo, FeeInfo, NodeClient, Supply};
use maya_api_gateway::webhooks::HttpTransport;
use tokio_tungstenite::tungstenite::Message;

const ALICE: &str = "aa00000000000000000000000000000000000000000000000000000000000001";
const BOB: &str = "bb00000000000000000000000000000000000000000000000000000000000002";

fn block(height: u64, sender: &str, recipient: &str) -> serde_json::Value {
    serde_json::json!({
        "height": height,
        "header": { "timestamp": height },
        "transactions": [{
            "txid": format!("{height:064x}"),
            "sender": sender,
            "outputs": [{ "recipient": recipient, "amount": "5" }],
        }],
        "raw": "00".repeat(1000),
    })
}

#[test]
fn a_block_yields_a_block_event_and_one_per_transaction() {
    let events = Event::from_block(9, &block(9, ALICE, BOB));
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].id, "block:9");
    assert_eq!(events[0].data["tx_count"], 1);
    assert!(events[0].data.get("raw").is_none(), "the raw block is not re-sent");
    assert_eq!(events[1].id, format!("tx:{:064x}", 9));
    assert_eq!(events[1].data["sender"], ALICE);
}

#[test]
fn the_address_filter_matches_sender_or_recipient_in_any_case() {
    let events = Event::from_block(1, &block(1, ALICE, BOB));
    let by = |address: &str| Filter {
        address: Some(address.to_string()),
        asset: None,
    }
    .validated()
    .unwrap();
    assert!(events.iter().all(|e| by(ALICE).matches(e)));
    assert!(events.iter().all(|e| by(&format!("0x{}", BOB.to_uppercase())).matches(e)));
    let carol = "cc".repeat(32);
    assert!(events.iter().all(|e| !by(&carol).matches(e)));
    assert!(events.iter().all(|e| Filter::default().matches(e)));
}

#[test]
fn the_asset_filter_reads_the_asset_field_and_native_otherwise() {
    let plain = Event::from_block(1, &block(1, ALICE, BOB));
    let token = serde_json::json!({
        "height": 2, "header": {},
        "transactions": [{ "txid": "ab", "sender": ALICE,
            "outputs": [{ "recipient": BOB, "amount": "1", "asset": "usdm" }] }],
    });
    let token = Event::from_block(2, &token);
    let asset = |a: &str| Filter { address: None, asset: Some(a.to_string()) }.validated().unwrap();
    assert!(plain.iter().all(|e| asset("native").matches(e)));
    assert!(plain.iter().all(|e| !asset("usdm").matches(e)));
    assert!(token.iter().all(|e| asset("usdm").matches(e)));
    assert!(token.iter().all(|e| !asset("native").matches(e)));
}

#[test]
fn an_oversized_transaction_is_sent_truncated() {
    let huge = serde_json::json!({
        "height": 1, "header": {},
        "transactions": [{ "txid": "ab", "sender": ALICE, "outputs": [],
            "summary": "x".repeat(events::MAX_EVENT_BYTES + 1) }],
    });
    let events = Event::from_block(1, &huge);
    assert_eq!(events[1].data["truncated"], true);
    assert_eq!(events[1].data["txid"], "ab");
    assert!(serde_json::to_vec(&events[1]).unwrap().len() < events::MAX_EVENT_BYTES);
}

/// A node whose tip and blocks the test controls.
struct ScriptedNode {
    tip: Mutex<u64>,
    fetched: Mutex<Vec<u64>>,
}

#[async_trait]
impl NodeClient for ScriptedNode {
    async fn get_balance(&self, _address: &str) -> Result<Balance, GatewayError> {
        Err(GatewayError::NotFound)
    }
    async fn get_block_by_height(&self, height: u64) -> Result<serde_json::Value, GatewayError> {
        self.fetched.lock().unwrap().push(height);
        Ok(block(height, ALICE, BOB))
    }
    async fn get_supply(&self) -> Result<Supply, GatewayError> {
        Err(GatewayError::NotFound)
    }
    async fn send_raw_transaction(&self, _raw: &str) -> Result<String, GatewayError> {
        Err(GatewayError::NotFound)
    }
    async fn get_fee_info(&self) -> Result<FeeInfo, GatewayError> {
        Err(GatewayError::NotFound)
    }
    async fn get_chain_info(&self) -> Result<ChainInfo, GatewayError> {
        Ok(ChainInfo {
            genesis: "00".repeat(32),
            chain_id: None,
            height: *self.tip.lock().unwrap(),
        })
    }
}

fn hub() -> Arc<Events> {
    Events::new(EventsConfig::default(), Arc::new(HttpTransport::new(false)))
}

#[tokio::test]
async fn the_poller_starts_at_the_tip_and_emits_each_new_block_once() {
    let node = ScriptedNode {
        tip: Mutex::new(10),
        fetched: Mutex::new(Vec::new()),
    };
    let hub = hub();
    let mut feed = hub.subscribe();
    let mut next = None;

    // History is not replayed: the first poll only records where the tip is.
    assert_eq!(events::poll_once(&node, &hub, &mut next).await.unwrap(), 0);
    *node.tip.lock().unwrap() = 12;
    assert_eq!(events::poll_once(&node, &hub, &mut next).await.unwrap(), 4);
    assert_eq!(events::poll_once(&node, &hub, &mut next).await.unwrap(), 0);
    assert_eq!(*node.fetched.lock().unwrap(), vec![11, 12]);

    let ids: Vec<String> = (0..4).map(|_| feed.try_recv().unwrap().id.clone()).collect();
    assert_eq!(ids[0], "block:11");
    assert_eq!(ids[2], "block:12");
}

#[tokio::test]
async fn the_poller_catches_up_in_bounded_steps() {
    let node = ScriptedNode {
        tip: Mutex::new(0),
        fetched: Mutex::new(Vec::new()),
    };
    let hub = hub();
    let mut next = None;
    events::poll_once(&node, &hub, &mut next).await.unwrap();
    *node.tip.lock().unwrap() = 10_000;
    events::poll_once(&node, &hub, &mut next).await.unwrap();
    assert_eq!(node.fetched.lock().unwrap().len(), events::MAX_BLOCKS_PER_POLL as usize);
    assert_eq!(next, Some(1 + events::MAX_BLOCKS_PER_POLL));
}

async fn serve(hub: Arc<Events>) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = events::router(hub).into_make_service_with_connect_info::<std::net::SocketAddr>();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    address
}

#[tokio::test]
async fn the_websocket_streams_only_matching_events() {
    let hub = hub();
    let address = serve(Arc::clone(&hub)).await;
    let url = format!("ws://{address}/v1/subscribe?address={ALICE}");
    let (mut socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    // Let the server side subscribe before publishing.
    for _ in 0..100 {
        if hub.stream_count() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    for event in Event::from_block(1, &block(1, BOB, BOB)) {
        hub.publish(event);
    }
    for event in Event::from_block(2, &block(2, BOB, ALICE)) {
        hub.publish(event);
    }
    let first = tokio::time::timeout(Duration::from_secs(5), socket.next()).await.unwrap().unwrap().unwrap();
    let Message::Text(text) = first else { panic!("text frame") };
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["id"], "block:2", "height 1 did not match: {json}");
}

#[tokio::test]
async fn a_bad_filter_or_too_many_streams_is_refused() {
    let hub = Events::new(
        EventsConfig {
            max_streams_per_caller: 1,
            ..EventsConfig::default()
        },
        Arc::new(HttpTransport::new(false)),
    );
    let address = serve(Arc::clone(&hub)).await;
    let bad = tokio_tungstenite::connect_async(format!("ws://{address}/v1/subscribe?address=xyz")).await;
    assert!(bad.is_err());

    let url = format!("ws://{address}/v1/subscribe");
    let _held = tokio_tungstenite::connect_async(url.clone()).await.unwrap();
    let refused = tokio_tungstenite::connect_async(url).await.expect_err("over the cap");
    assert!(refused.to_string().contains("429"), "{refused}");
}
