//! The SDK against the **real** gateway router (`maya_api_gateway::app`),
//! served on loopback, with a recorded node behind it.
//!
//! `client_tests.rs` checks the client's retry and decode rules against a
//! hand-written HTTP server; this file checks that the paths, bodies, status
//! codes and field names the SDK expects are the ones the gateway actually
//! serves. A gateway route renamed, an amount turned back into a number, or a
//! 404 turned into a 200 fails here.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use maya_api_gateway::error::GatewayError;
use maya_api_gateway::node::{Balance, ChainInfo, FeeInfo, NodeClient, Supply};
use maya2c_sdk::{Client, Error, Network, encode_address};
use primitive_types::U256;
use serde_json::{Value, json};

const RICH: [u8; 32] = [0x5A; 32];
/// 2^128 + 1: exact only if no layer turns it into a float.
const RICH_BALANCE: &str = "340282366920938463463374607431768211457";
/// A raw transaction the recorded node refuses.
const STALE: &str = "dead";
const TIP: u64 = 3;

fn txid(height: u64) -> String {
    format!("{height:02x}").repeat(32)
}

/// A recorded node; remembers what the gateway forwarded to it.
#[derive(Default)]
struct Node {
    submitted: Mutex<Vec<String>>,
    asked: Mutex<Vec<String>>,
}

#[async_trait]
impl NodeClient for Node {
    async fn get_balance(&self, address: &str) -> Result<Balance, GatewayError> {
        self.asked.lock().unwrap().push(address.to_string());
        Ok(if address == hex::encode(RICH) {
            Balance {
                balance: RICH_BALANCE.into(),
                nonce: 4,
            }
        } else {
            Balance {
                balance: "0".into(),
                nonce: 0,
            }
        })
    }

    async fn get_block_by_height(&self, height: u64) -> Result<Value, GatewayError> {
        if height > TIP {
            return Ok(Value::Null);
        }
        Ok(
            json!({ "height": height, "header": { "id": "aa" }, "transactions": [{ "txid": txid(height) }] }),
        )
    }

    async fn get_supply(&self) -> Result<Supply, GatewayError> {
        Ok(Supply {
            circulating: RICH_BALANCE.into(),
            total: "19687500000000000000000000000000".into(),
        })
    }

    async fn send_raw_transaction(&self, raw: &str) -> Result<String, GatewayError> {
        if raw == STALE {
            return Err(GatewayError::Rejected(
                "invalid nonce: expected 4, got 3".into(),
            ));
        }
        self.submitted.lock().unwrap().push(raw.to_string());
        Ok(txid(TIP))
    }

    async fn get_fee_info(&self) -> Result<FeeInfo, GatewayError> {
        Ok(FeeInfo {
            active: true,
            base_fee: 7,
            collector: "00".repeat(32),
        })
    }

    async fn get_chain_info(&self) -> Result<ChainInfo, GatewayError> {
        Ok(ChainInfo {
            genesis: "ab".repeat(32),
            chain_id: Some("maya-devnet".into()),
            height: TIP,
        })
    }
}

/// Serves the gateway over `node` on a loopback port; returns its base URL.
fn serve(node: Arc<Node>) -> String {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap()).unwrap();
            axum::serve(listener, maya_api_gateway::app(node))
                .await
                .unwrap();
        });
    });
    format!("http://{}", rx.recv().unwrap())
}

fn client() -> (Client, Arc<Node>) {
    let node = Arc::new(Node::default());
    let url = serve(Arc::clone(&node));
    (
        Client::with_timeout(&url, Duration::from_secs(10)).unwrap(),
        node,
    )
}

fn api_status(e: &Error) -> u16 {
    match e {
        Error::Api(a) => a.status,
        other => panic!("expected a gateway error, got {other:?}"),
    }
}

#[test]
fn every_read_decodes_what_the_real_gateway_serves() {
    let (c, _) = client();
    assert_eq!(c.health().unwrap().status, "ok");
    let chain = c.chain_info().unwrap();
    assert_eq!(
        (chain.height, chain.chain_id.as_deref()),
        (TIP, Some("maya-devnet"))
    );
    assert_eq!(chain.genesis, "ab".repeat(32));
    let fees = c.fee_info().unwrap();
    assert!(fees.active);
    assert_eq!(fees.base_fee, 7);
    let supply = c.supply().unwrap();
    assert_eq!(
        supply.circulating,
        U256::from_dec_str(RICH_BALANCE).unwrap()
    );
    assert_eq!(
        supply.total,
        U256::from_dec_str("19687500000000000000000000000000").unwrap()
    );
    let block = c.block(2).unwrap();
    assert_eq!(block["transactions"][0]["txid"], txid(2));
}

#[test]
fn a_bech32m_and_a_hex_address_reach_the_node_as_the_same_hex() {
    let (c, node) = client();
    let text = encode_address(Network::Devnet, &RICH);
    let by_text = c.balance(&text).unwrap();
    let by_hex = c.balance(&format!("0x{}", hex::encode(RICH))).unwrap();
    assert_eq!(by_text, by_hex);
    assert_eq!(
        by_text.balance,
        U256::from_dec_str(RICH_BALANCE).unwrap(),
        "exact above 2^128"
    );
    assert_eq!(by_text.nonce, 4);
    assert_eq!(
        *node.asked.lock().unwrap(),
        [hex::encode(RICH), hex::encode(RICH)]
    );
}

#[test]
fn a_mistyped_address_never_reaches_the_gateway() {
    let (c, node) = client();
    let mut text = encode_address(Network::Devnet, &RICH);
    let last = text.pop().unwrap();
    text.push(if last == 'q' { 'p' } else { 'q' });
    for bad in [text.as_str(), "", "0x1234", &"g".repeat(64)] {
        assert!(
            matches!(c.balance(bad), Err(Error::InvalidAddress(_))),
            "{bad}"
        );
    }
    assert!(
        node.asked.lock().unwrap().is_empty(),
        "nothing was forwarded"
    );
}

#[test]
fn a_missing_block_is_a_404_not_a_null_block() {
    let (c, _) = client();
    assert_eq!(api_status(&c.block(TIP + 1).unwrap_err()), 404);
}

#[test]
fn a_submission_reaches_the_node_and_a_refusal_is_a_400_with_the_gateways_words() {
    let (c, node) = client();
    assert_eq!(c.send_raw_transaction("00ff").unwrap(), txid(TIP));
    assert_eq!(*node.submitted.lock().unwrap(), ["00ff"]);

    let refused = c.send_raw_transaction(STALE).unwrap_err();
    assert_eq!(api_status(&refused), 400);
    // The client sees the gateway's summary, not the node's internal text.
    let Error::Api(e) = refused else {
        unreachable!()
    };
    assert!(!e.detail.contains("expected 4"), "{e:?}");

    assert_eq!(
        api_status(&c.send_raw_transaction("").unwrap_err()),
        400,
        "empty raw"
    );
    assert_eq!(
        node.submitted.lock().unwrap().len(),
        1,
        "the refused ones were not queued twice"
    );
}

#[test]
fn a_sealed_submission_is_validated_by_the_gateway_before_the_node() {
    let (c, node) = client();
    let ok = c.send_sealed_transaction("c0ffee").unwrap();
    assert_eq!((ok.id, ok.bytes), (txid(TIP), 3));
    for bad in ["", "abc", "zz"] {
        assert_eq!(
            api_status(&c.send_sealed_transaction(bad).unwrap_err()),
            400,
            "{bad:?}"
        );
    }
    let too_big = "00".repeat(maya_api_gateway::sealed::MAX_SEALED_PAYLOAD_BYTES + 1);
    assert_eq!(
        api_status(&c.send_sealed_transaction(&too_big).unwrap_err()),
        413
    );
    assert_eq!(*node.submitted.lock().unwrap(), ["c0ffee"]);
}

#[test]
fn wait_for_transaction_finds_the_block_through_the_gateway() {
    let (c, _) = client();
    // The SDK scans from the tip it first sees; the tip block carries it.
    let upper = txid(TIP).to_ascii_uppercase();
    assert_eq!(
        c.wait_for_transaction(&format!("0x{upper}"), Duration::from_secs(5))
            .unwrap(),
        TIP
    );
    assert!(matches!(
        c.wait_for_transaction(&"ee".repeat(32), Duration::from_millis(300)),
        Err(Error::Timeout)
    ));
}
