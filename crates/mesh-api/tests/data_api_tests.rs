//! The Data API end to end: Mesh request -> router -> `RpcNode` -> a JSON-RPC
//! node over real HTTP on loopback.
//!
//! The mock node answers in the node's *exact* wire shapes
//! (`crates/node/src/rpc/types.rs`): balances are base-10 strings (ADR-046),
//! the account read is a flattened `AccountInfo`, headers and transactions
//! carry fields this service ignores, and a missing block is JSON-RPC error
//! -32001. A service that decoded balances as JSON numbers passed every test
//! that used a hand-built `Node`, and failed every balance read against the
//! real node; these tests go through the wire so that cannot recur.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use http_body_util::BodyExt;
use maya_mesh_api::node::{RpcNode, parse_amount};
use serde_json::{Value, json};
use tower::ServiceExt;

const NETWORK: &str = "maya-test";
const ALICE: &str = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1";
const BOB: &str = "b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0";
/// 5,000,000 coins at 24 decimals: far beyond `u64`, an ordinary balance.
const ALICE_GENESIS: &str = "5000000000000000000000000000000";
/// After sending 2,000,000 coins and burning a 0.001-coin fee.
const ALICE_AFTER: &str = "2999999999000000000000000000000";
const BOB_AFTER: &str = "2000000000000000000000000000000";
const TIP: u64 = 2;
const NOT_FOUND: i64 = -32_001;
/// 2^256 - 1.
const MAX_DEC: &str =
    "115792089237316195423570985008687907853269984665640564039457584007913129639935";

type Reply = Result<Value, (i64, String)>;
type Handler = Arc<dyn Fn(&str, &[Value]) -> Reply + Send + Sync>;

fn id(height: u64) -> String {
    format!("{height:02x}").repeat(32)
}

/// A two-block chain: block 1 moves value and burns a fee, block 2 moves
/// nothing.
fn chain(method: &str, params: &[Value]) -> Reply {
    let height = |i: usize| params.get(i).and_then(Value::as_u64);
    let address = || {
        params
            .first()
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let balance = |who: &str, at: u64| match (who, at) {
        (ALICE, 0) => ALICE_GENESIS,
        (ALICE, _) => ALICE_AFTER,
        (BOB, 0) => "0",
        (BOB, _) => BOB_AFTER,
        _ => "0",
    };
    match method {
        "get_block_by_height" => {
            let h = height(0).ok_or((-32_602, "height".to_string()))?;
            if h > TIP {
                return Err((NOT_FOUND, format!("no block at height {h}")));
            }
            let prev = if h == 0 { "00".repeat(32) } else { id(h - 1) };
            let txs = if h == 1 {
                json!([{ "txid": "f1".repeat(32), "inputs": [], "outputs": [], "fee": "1000000000000000000000" }])
            } else {
                json!([])
            };
            Ok(json!({
                "height": h,
                "header": {
                    "id": id(h), "prev_hash": prev, "state_root": "11".repeat(32),
                    "timestamp": 1_790_000_000 + h, "nonce": 0,
                    "difficulty_target": "ff".repeat(32), "tx_root": "22".repeat(32)
                },
                "transactions": txs,
                "raw": "00"
            }))
        }
        "get_balance_changes" => match height(0) {
            Some(1) => Ok(json!({
                "height": 1, "block_id": id(1),
                "changes": [
                    { "address": ALICE, "before": ALICE_GENESIS, "after": ALICE_AFTER },
                    { "address": BOB, "before": "0", "after": BOB_AFTER }
                ]
            })),
            Some(2) => Ok(json!({ "height": 2, "block_id": id(2), "changes": [] })),
            _ => Err((NOT_FOUND, "genesis or pruned".to_string())),
        },
        "get_account_at_tip" => {
            let who = address();
            Ok(json!({
                "address": who, "balance": balance(&who, TIP), "nonce": 1,
                "height": TIP, "block_id": id(TIP)
            }))
        }
        "get_balance_at_height" => {
            let who = address();
            let h = height(1).ok_or((-32_602, "height".to_string()))?;
            if h > TIP {
                return Err((NOT_FOUND, "above the tip".to_string()));
            }
            Ok(
                json!({ "address": who, "balance": balance(&who, h), "height": h, "block_id": id(h) }),
            )
        }
        other => Err((-32_601, format!("unknown method {other}"))),
    }
}

/// Serves `handler` as a JSON-RPC node on a loopback port; returns its URL.
async fn serve_node(handler: Handler) -> String {
    let app = Router::new().route(
        "/",
        post(move |Json(req): Json<Value>| {
            let handler = Arc::clone(&handler);
            async move {
                let params = req["params"].as_array().cloned().unwrap_or_default();
                let reply = handler(req["method"].as_str().unwrap_or(""), &params);
                Json(match reply {
                    Ok(result) => json!({ "jsonrpc": "2.0", "id": req["id"], "result": result }),
                    Err((code, message)) => json!({
                        "jsonrpc": "2.0", "id": req["id"],
                        "error": { "code": code, "message": message }
                    }),
                })
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

async fn mesh_over(handler: Handler) -> Router {
    let url = serve_node(handler).await;
    maya_mesh_api::router(Arc::new(RpcNode::connect(&url).unwrap()), NETWORK)
}

async fn mesh() -> Router {
    mesh_over(Arc::new(chain)).await
}

fn network() -> Value {
    json!({ "blockchain": "maya2c", "network": NETWORK })
}

async fn call(app: &Router, path: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::post(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

/// Asserts a Mesh error with `code` and `retriable`.
fn assert_error(reply: &(StatusCode, Value), code: u64, retriable: bool) {
    assert_eq!(reply.0, StatusCode::INTERNAL_SERVER_ERROR, "{}", reply.1);
    assert_eq!(reply.1["code"], code, "{}", reply.1);
    assert_eq!(reply.1["retriable"], retriable, "{}", reply.1);
}

fn ops(block: &Value) -> Vec<(String, String)> {
    block["block"]["transactions"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|t| t["operations"].as_array().unwrap().clone())
        .map(|o| {
            (
                o["account"]["address"].as_str().unwrap().to_string(),
                o["amount"]["value"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// Signed decimal string -> (negative, magnitude). Test-only, for sums.
fn signed(value: &str) -> (bool, maya_amount::Amount) {
    let (neg, digits) = value
        .strip_prefix('-')
        .map_or((false, value), |d| (true, d));
    (neg, parse_amount(digits).unwrap())
}

#[tokio::test]
async fn network_list_options_and_status_describe_the_node() {
    let app = mesh().await;
    let (status, list) = call(&app, "/network/list", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["network_identifiers"], json!([network()]));

    let (_, options) = call(
        &app,
        "/network/options",
        json!({ "network_identifier": network() }),
    )
    .await;
    assert_eq!(options["allow"]["historical_balance_lookup"], true);
    let codes: Vec<u64> = options["allow"]["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["code"].as_u64().unwrap())
        .collect();
    assert_eq!(
        codes,
        [1, 2, 3, 4],
        "every error the API returns is declared"
    );

    let (status, s) = call(
        &app,
        "/network/status",
        json!({ "network_identifier": network() }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{s}");
    assert_eq!(
        s["current_block_identifier"],
        json!({ "index": TIP, "hash": id(TIP) })
    );
    assert_eq!(
        s["genesis_block_identifier"],
        json!({ "index": 0, "hash": id(0) })
    );
    assert_eq!(s["current_block_timestamp"], (1_790_000_000 + TIP) * 1_000);
}

#[tokio::test]
async fn every_endpoint_refuses_another_networks_identifier() {
    let app = mesh().await;
    let wrong = json!({ "blockchain": "maya2c", "network": "maya-mainnet" });
    let other_chain = json!({ "blockchain": "bitcoin", "network": NETWORK });
    for bad_id in [wrong, other_chain] {
        for (path, extra) in [
            ("/network/options", json!({})),
            ("/network/status", json!({})),
            ("/block", json!({ "block_identifier": { "index": 1 } })),
            (
                "/account/balance",
                json!({ "account_identifier": { "address": ALICE } }),
            ),
            ("/mempool", json!({})),
        ] {
            let mut body = extra;
            body["network_identifier"] = bad_id.clone();
            let reply = call(&app, path, body).await;
            assert_error(&reply, 1, false);
        }
    }
}

#[tokio::test]
async fn a_block_carries_256_bit_operations_that_reconcile_with_balances() {
    let app = mesh().await;
    let (status, b1) = call(
        &app,
        "/block",
        json!({ "network_identifier": network(), "block_identifier": { "index": 1 } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{b1}");
    assert_eq!(
        b1["block"]["parent_block_identifier"],
        json!({ "index": 0, "hash": id(0) })
    );
    let txs = b1["block"]["transactions"].as_array().unwrap();
    assert_eq!(
        txs.len(),
        2,
        "the signed transaction plus the balance-changes transaction"
    );
    assert_eq!(
        txs[1]["transaction_identifier"]["hash"],
        format!("block-balance-changes:{}", id(1))
    );
    assert_eq!(
        ops(&b1),
        [
            (
                ALICE.to_string(),
                "-2000000001000000000000000000000".to_string()
            ),
            (BOB.to_string(), BOB_AFTER.to_string()),
        ]
    );
    assert_eq!(
        txs[1]["operations"][0]["amount"]["currency"]["decimals"],
        24
    );

    // What mesh-cli check:data does: bootstrap + every operation = balance.
    for (who, genesis) in [(ALICE, ALICE_GENESIS), (BOB, "0")] {
        let mut total = parse_amount(genesis).unwrap();
        for h in 1..=TIP {
            let (_, b) = call(
                &app,
                "/block",
                json!({ "network_identifier": network(), "block_identifier": { "index": h } }),
            )
            .await;
            for (_, value) in ops(&b).into_iter().filter(|(a, _)| a == who) {
                let (neg, mag) = signed(&value);
                total = if neg {
                    total.checked_sub(mag)
                } else {
                    total.checked_add(mag)
                }
                .unwrap();
            }
        }
        let (_, bal) = call(
            &app,
            "/account/balance",
            json!({ "network_identifier": network(), "account_identifier": { "address": who } }),
        )
        .await;
        assert_eq!(
            bal["balances"][0]["value"],
            total.to_string(),
            "{who} reconciles"
        );
        assert_eq!(
            bal["block_identifier"],
            json!({ "index": TIP, "hash": id(TIP) })
        );
    }
}

#[tokio::test]
async fn genesis_and_an_empty_block_have_no_balance_changes_transaction() {
    let app = mesh().await;
    for h in [0, TIP] {
        let (status, b) = call(
            &app,
            "/block",
            json!({ "network_identifier": network(), "block_identifier": { "index": h } }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{b}");
        assert!(ops(&b).is_empty(), "height {h}: {b}");
    }
    let (_, g) = call(
        &app,
        "/block",
        json!({ "network_identifier": network(), "block_identifier": { "index": 0 } }),
    )
    .await;
    assert_eq!(
        g["block"]["parent_block_identifier"],
        g["block"]["block_identifier"]
    );
}

#[tokio::test]
async fn block_lookup_by_tip_hash_and_missing_height() {
    let app = mesh().await;
    let block = |bid: Value| json!({ "network_identifier": network(), "block_identifier": bid });

    let (_, tip) = call(&app, "/block", block(json!({}))).await;
    assert_eq!(
        tip["block"]["block_identifier"]["index"], TIP,
        "no identifier = the tip"
    );

    let (status, ok) = call(&app, "/block", block(json!({ "index": 1, "hash": id(1) }))).await;
    assert_eq!(status, StatusCode::OK, "{ok}");

    // An index with another block's hash is a different block: not found.
    assert_error(
        &call(&app, "/block", block(json!({ "index": 1, "hash": id(2) }))).await,
        2,
        false,
    );
    // A hash alone needs an index the node does not serve.
    assert_error(
        &call(&app, "/block", block(json!({ "hash": id(1) }))).await,
        4,
        false,
    );
    // Above the tip: the node's -32001 is Mesh's "not found", not "unavailable".
    assert_error(
        &call(&app, "/block", block(json!({ "index": TIP + 1 }))).await,
        2,
        false,
    );
}

#[tokio::test]
async fn a_reorg_between_the_two_reads_is_refused_and_retriable() {
    let reorged: Handler = Arc::new(|method: &str, params: &[Value]| {
        let mut reply = chain(method, params)?;
        if method == "get_balance_changes" {
            reply["block_id"] = json!("ee".repeat(32));
        }
        Ok(reply)
    });
    let app = mesh_over(reorged).await;
    let reply = call(
        &app,
        "/block",
        json!({ "network_identifier": network(), "block_identifier": { "index": 1 } }),
    )
    .await;
    assert_error(&reply, 3, true);
}

#[tokio::test]
async fn historical_balance_reads_the_height_asked_and_checks_its_hash() {
    let app = mesh().await;
    let at = |bid: Value| {
        json!({
            "network_identifier": network(),
            "account_identifier": { "address": ALICE },
            "block_identifier": bid
        })
    };
    let (status, g) = call(&app, "/account/balance", at(json!({ "index": 0 }))).await;
    assert_eq!(status, StatusCode::OK, "{g}");
    assert_eq!(g["balances"][0]["value"], ALICE_GENESIS);
    assert_eq!(
        g["balances"][0]["currency"],
        json!({ "symbol": "MAYA", "decimals": 24 })
    );
    assert_eq!(g["block_identifier"], json!({ "index": 0, "hash": id(0) }));

    let (_, one) = call(
        &app,
        "/account/balance",
        at(json!({ "index": 1, "hash": id(1) })),
    )
    .await;
    assert_eq!(one["balances"][0]["value"], ALICE_AFTER);

    assert_error(
        &call(
            &app,
            "/account/balance",
            at(json!({ "index": 1, "hash": id(0) })),
        )
        .await,
        2,
        false,
    );
    assert_error(
        &call(&app, "/account/balance", at(json!({ "hash": id(1) }))).await,
        4,
        false,
    );
    assert_error(
        &call(&app, "/account/balance", at(json!({ "index": TIP + 1 }))).await,
        2,
        false,
    );
}

#[tokio::test]
async fn a_malformed_balance_from_the_node_is_unavailable_never_a_number() {
    // A balance this service misreads would make a custodian credit or debit
    // the wrong amount; every one of these must be an error, none a value.
    for bad in [
        json!(""),
        json!("-5"),
        json!("+5"),
        json!(" 5"),
        json!("5 "),
        json!("0x10"),
        json!("1e30"),
        json!("1.5"),
        json!(5),
        json!(null),
        json!(format!("{MAX_DEC}0")),
        json!("115792089237316195423570985008687907853269984665640564039457584007913129639936"),
    ] {
        let lying: Handler = Arc::new(move |method: &str, params: &[Value]| {
            let mut reply = chain(method, params)?;
            if method == "get_account_at_tip" {
                reply["balance"] = bad.clone();
            }
            Ok(reply)
        });
        let app = mesh_over(lying).await;
        let reply = call(
            &app,
            "/account/balance",
            json!({ "network_identifier": network(), "account_identifier": { "address": ALICE } }),
        )
        .await;
        assert_error(&reply, 3, true);
    }
}

#[tokio::test]
async fn the_largest_balance_the_node_can_hold_is_served_exactly() {
    let rich: Handler = Arc::new(|method: &str, params: &[Value]| {
        let mut reply = chain(method, params)?;
        if method == "get_account_at_tip" {
            reply["balance"] = json!(MAX_DEC);
        }
        Ok(reply)
    });
    let app = mesh_over(rich).await;
    let (status, bal) = call(
        &app,
        "/account/balance",
        json!({ "network_identifier": network(), "account_identifier": { "address": ALICE } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{bal}");
    assert_eq!(bal["balances"][0]["value"], MAX_DEC);
}

#[tokio::test]
async fn a_node_that_is_down_is_a_retriable_error() {
    // Bind then drop: a loopback port nothing listens on.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let node = RpcNode::connect(&format!("http://127.0.0.1:{port}")).unwrap();
    let app = maya_mesh_api::router(Arc::new(node), NETWORK);
    for (path, body) in [
        (
            "/network/status",
            json!({ "network_identifier": network() }),
        ),
        (
            "/block",
            json!({ "network_identifier": network(), "block_identifier": { "index": 1 } }),
        ),
    ] {
        assert_error(&call(&app, path, body).await, 3, true);
    }
}

#[tokio::test]
async fn construction_and_transaction_lookup_are_declared_unsupported() {
    let app = mesh().await;
    let reply = call(
        &app,
        "/block/transaction",
        json!({ "network_identifier": network() }),
    )
    .await;
    assert_error(&reply, 4, false);
    let (status, m) = call(&app, "/mempool", json!({ "network_identifier": network() })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(m, json!({ "transaction_identifiers": [] }));
    // No Construction route exists at all (Mesh has no PQ signature type).
    let req = Request::post("/construction/derive")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "network_identifier": network() }).to_string(),
        ))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
}

#[test]
fn parse_amount_takes_digits_only_and_stops_at_2_pow_256() {
    assert_eq!(parse_amount("0"), Some(maya_amount::Amount::from_u64(0)));
    assert_eq!(
        parse_amount("18446744073709551616"),
        Some(maya_amount::Amount([0, 1, 0, 0]))
    );
    assert_eq!(
        parse_amount(MAX_DEC),
        Some(maya_amount::Amount([u64::MAX; 4]))
    );
    let refused: BTreeMap<&str, Option<maya_amount::Amount>> = [
        "",
        "-1",
        "+1",
        " 1",
        "1_000",
        "0x1",
        "١", // Arabic-Indic one: not ASCII
        "115792089237316195423570985008687907853269984665640564039457584007913129639936",
    ]
    .into_iter()
    .map(|t| (t, parse_amount(t)))
    .collect();
    assert!(refused.values().all(Option::is_none), "{refused:?}");
}
