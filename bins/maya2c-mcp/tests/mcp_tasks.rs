//! The MCP server, started as an assistant would start it (stdio), against a
//! mock node that answers each read method with canned data and records every
//! method it receives (ADR-084).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use custom_l1_node::TxOutput;
use custom_l1_node::core::Transaction;
use custom_l1_node::core::transaction::ChainTag;
use custom_l1_node::crypto::hybrid::generate_signing_key;
use maya_address::Network;
use primitive_types::U256;
use serde_json::{Value, json};

const GENESIS: [u8; 32] = [0x11; 32];
const CHAIN_ID: &str = "maya-testnet-1";
const COLLECTOR: [u8; 32] = [0x22; 32];
const TXID: [u8; 32] = [0x33; 32];
const BASE_FEE: u64 = 1000;
/// 10 coins in base units (24 decimals).
const BALANCE: &str = "10000000000000000000000000";
const NONCE: u64 = 3;
/// Mirrors `READ_METHODS` in `src/node.rs`: the only methods the server may send.
const READ_METHODS: [&str; 6] = [
    "get_chain_info",
    "get_balance",
    "get_block_by_height",
    "get_validators",
    "get_supply",
    "get_fee_info",
];

type Seen = Arc<Mutex<Vec<String>>>;

/// Reads one HTTP request and returns its JSON body.
fn read_body(s: &mut TcpStream) -> Value {
    let mut req = Vec::new();
    let mut buf = [0u8; 4096];
    while let Ok(n) = s.read(&mut buf) {
        if n == 0 {
            break;
        }
        req.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&req);
        if let Some(end) = text.find("\r\n\r\n") {
            let len = text[..end]
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            if req.len() >= end + 4 + len {
                break;
            }
        }
    }
    let text = String::from_utf8_lossy(&req).into_owned();
    let body = text.split_once("\r\n\r\n").map_or("", |(_, b)| b);
    serde_json::from_str(body).unwrap_or(Value::Null)
}

fn canned(method: &str, params: &Value) -> Value {
    match method {
        "get_chain_info" => {
            json!({"genesis": hex::encode(GENESIS), "chain_id": CHAIN_ID, "height": 7})
        }
        "get_block_by_height" => json!({
            "height": params[0],
            "header": {"prev": "00"},
            "transactions": [{"txid": hex::encode(TXID), "sender": hex::encode([9u8; 32])}],
            "raw": "deadbeef"
        }),
        "get_balance" => json!({"address": params[0], "balance": BALANCE, "nonce": NONCE}),
        "get_validators" => json!([{"id": "v1"}]),
        "get_supply" => json!({"total": "21", "circulating": "5", "burned": "0"}),
        "get_fee_info" => {
            json!({"active": true, "base_fee": BASE_FEE.to_string(), "collector": hex::encode(COLLECTOR)})
        }
        _ => Value::Null,
    }
}

/// A fake node. `fail` names one method that answers with a JSON-RPC error.
fn mock_node(fail: Option<&'static str>) -> (String, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let seen: Seen = Arc::default();
    let log = Arc::clone(&seen);
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let req = read_body(&mut stream);
            let method = req["method"].as_str().unwrap_or("").to_string();
            log.lock().unwrap().push(method.clone());
            let reply = if fail == Some(method.as_str()) {
                json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32000, "message": "node is busy"}})
            } else {
                json!({"jsonrpc": "2.0", "id": 1, "result": canned(&method, &req["params"])})
            };
            let text = reply.to_string();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{text}",
                text.len()
            );
        }
    });
    (addr, seen)
}

fn session(rpc: Option<&str>, requests: &[Value]) -> Vec<Value> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_maya2c-mcp"));
    if let Some(addr) = rpc {
        cmd.args(["--rpc", addr]);
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for r in requests {
        writeln!(stdin, "{r}").unwrap();
    }
    drop(stdin);
    let replies: Vec<Value> = BufReader::new(child.stdout.take().unwrap())
        .lines()
        .map(|l| serde_json::from_str(&l.unwrap()).unwrap())
        .collect();
    child.wait().unwrap();
    replies
}

fn tool(id: u64, name: &str, arguments: &Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": arguments}})
}

fn tools_list() -> Value {
    json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})
}

/// The tool's text and whether it reported an error.
fn text(reply: &Value) -> (String, bool) {
    (
        reply["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string(),
        reply["result"]["isError"].as_bool().unwrap(),
    )
}

/// The tool's text parsed as JSON; panics if the tool reported an error.
fn ok_json(reply: &Value) -> Value {
    let (t, err) = text(reply);
    assert!(!err, "tool failed: {t}");
    serde_json::from_str(&t).unwrap()
}

fn vector(id: &str) -> String {
    let doc: Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../spec/tests/encoding.json"
        ))
        .unwrap(),
    )
    .unwrap();
    doc["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id)
        .unwrap()["bytes"]
        .as_str()
        .unwrap()
        .to_string()
}

fn tool_names() -> Vec<String> {
    let reply = &session(None, &[tools_list()])[0];
    reply["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

fn maya1(id: [u8; 32]) -> String {
    maya_address::encode(Network::Testnet, &id)
}

/// Size of a signed transfer with the same shape the server drafts.
fn real_signed_size() -> usize {
    let outputs = vec![
        TxOutput {
            amount: U256::zero(),
            recipient: [1; 32],
        },
        TxOutput {
            amount: U256::zero(),
            recipient: COLLECTOR,
        },
    ];
    let mut tx = Transaction::new(vec![], outputs, NONCE);
    tx.sign(
        &generate_signing_key().unwrap(),
        &ChainTag::from_genesis(GENESIS),
    )
    .unwrap();
    tx.to_bytes().len()
}

#[test]
fn starts_and_lists_eleven_tools_none_of_which_signs_or_submits() {
    let replies = session(
        None,
        &[
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
            tools_list(),
        ],
    );
    assert_eq!(replies[0]["result"]["serverInfo"]["name"], "maya2c-mcp");
    let names = tool_names();
    assert_eq!(names.len(), 11, "{names:?}");
    for name in &names {
        for banned in ["sign", "submit", "send", "broadcast", "rpc"] {
            assert!(!name.contains(banned), "tool {name} looks like a write");
        }
    }
}

#[test]
fn no_tool_can_make_the_server_send_a_write_to_the_node() {
    let (addr, seen) = mock_node(None);
    let mut requests: Vec<Value> = tool_names()
        .iter()
        .enumerate()
        .map(|(i, name)| tool(10 + i as u64, name, &json!({})))
        .collect();
    requests.push(tool(
        50,
        "rpc_call",
        &json!({"method": "send_raw_transaction", "params": ["00"]}),
    ));
    requests.push(tool(
        51,
        "draft_transfer",
        &json!({"from": maya1([5; 32]), "to": hex::encode([1; 32]), "amount": "1.5"}),
    ));
    let replies = session(Some(&addr), &requests);
    let (t, err) = text(replies.last().unwrap());
    assert!(!err, "{t}");
    let (t, err) = text(&replies[replies.len() - 2]);
    assert!(err && t.contains("unknown tool"), "rpc_call is gone: {t}");
    let sent = seen.lock().unwrap().clone();
    assert!(!sent.is_empty());
    for method in &sent {
        assert!(READ_METHODS.contains(&method.as_str()), "sent {method}");
    }
    assert!(
        !sent
            .iter()
            .any(|m| m.contains("send") || m.contains("submit"))
    );
}

#[test]
fn get_height_reports_height_and_chain_tag() {
    let (addr, _) = mock_node(None);
    let reply = &session(Some(&addr), &[tool(1, "get_height", &json!({}))])[0];
    let v = ok_json(reply);
    assert_eq!(v["height"], 7);
    assert_eq!(v["chain_id"], CHAIN_ID);
    assert_eq!(v["genesis"], hex::encode(GENESIS));
}

#[test]
fn get_block_lists_txids_and_drops_the_raw_block() {
    let (addr, _) = mock_node(None);
    let v = ok_json(&session(Some(&addr), &[tool(1, "get_block", &json!({"height": 7}))])[0]);
    assert_eq!(v["height"], 7);
    assert_eq!(v["tx_count"], 1);
    assert_eq!(v["txids"][0], hex::encode(TXID));
    assert!(
        v.get("raw").is_none(),
        "raw block bytes stay out of the answer"
    );
}

#[test]
fn get_tx_finds_a_transaction_in_the_named_block_only() {
    let (addr, _) = mock_node(None);
    let found = ok_json(
        &session(
            Some(&addr),
            &[tool(
                1,
                "get_tx",
                &json!({"txid": hex::encode(TXID), "height": 7}),
            )],
        )[0],
    );
    assert_eq!(found["found"], true);
    assert_eq!(found["transaction"]["txid"], hex::encode(TXID));
    let other = "44".repeat(32);
    let absent = ok_json(
        &session(
            Some(&addr),
            &[tool(1, "get_tx", &json!({"txid": other, "height": 7}))],
        )[0],
    );
    assert_eq!(absent["found"], false);
    let (t, err) = text(
        &session(
            Some(&addr),
            &[tool(1, "get_tx", &json!({"txid": "xyz", "height": 7}))],
        )[0],
    );
    assert!(err && t.contains("txid"), "{t}");
}

#[test]
fn get_balance_takes_maya1_or_hex_and_asks_the_node_in_hex() {
    let (addr, seen) = mock_node(None);
    let id = [5u8; 32];
    let by_text = ok_json(
        &session(
            Some(&addr),
            &[tool(1, "get_balance", &json!({"address": maya1(id)}))],
        )[0],
    );
    let by_hex = ok_json(
        &session(
            Some(&addr),
            &[tool(1, "get_balance", &json!({"address": hex::encode(id)}))],
        )[0],
    );
    assert_eq!(by_text, by_hex);
    assert_eq!(by_text["balance"], BALANCE);
    assert_eq!(by_text["nonce"], NONCE);
    assert_eq!(by_text["address"], hex::encode(id));
    let (t, err) = text(
        &session(
            Some(&addr),
            &[tool(1, "get_balance", &json!({"address": "maya1qqqq"}))],
        )[0],
    );
    assert!(err && t.contains("not a maya1 address"), "{t}");
    assert!(seen.lock().unwrap().iter().all(|m| m == "get_balance"));
}

#[test]
fn get_validators_and_get_supply_pass_the_node_answer_through() {
    let (addr, _) = mock_node(None);
    let v = ok_json(&session(Some(&addr), &[tool(1, "get_validators", &json!({}))])[0]);
    assert_eq!(v["validators"][0]["id"], "v1");
    let s = ok_json(&session(Some(&addr), &[tool(1, "get_supply", &json!({}))])[0]);
    assert_eq!(s["total"], "21");
}

#[test]
fn estimate_fee_is_base_fee_times_the_real_signed_size() {
    let size = real_signed_size();
    let (addr, _) = mock_node(None);
    let v = ok_json(&session(Some(&addr), &[tool(1, "estimate_fee", &json!({}))])[0]);
    assert_eq!(v["transfer_size_bytes"], size);
    assert_eq!(v["fee_base_units"], (BASE_FEE * size as u64).to_string());
    assert_eq!(v["fees_active"], true);
}

#[test]
fn draft_transfer_returns_an_unsigned_transaction_that_a_wallet_can_finish() {
    let size = real_signed_size();
    let (addr, _) = mock_node(None);
    let from = [5u8; 32];
    let v = ok_json(
        &session(
            Some(&addr),
            &[tool(
                1,
                "draft_transfer",
                &json!({"from": maya1(from), "to": hex::encode([1u8; 32]), "amount": "1.5"}),
            )],
        )[0],
    );
    assert_eq!(v["signed"], false);
    assert_eq!(v["amount_base_units"], "1500000000000000000000000");
    assert_eq!(v["fee_base_units"], (BASE_FEE * size as u64).to_string());
    assert_eq!(v["sufficient_balance"], true);
    assert_eq!(v["chain_tag"], hex::encode(GENESIS));
    let summary = v["summary"].as_str().unwrap();
    assert!(
        summary.contains("Send 1.5 coins") && summary.contains(&maya1(from)),
        "{summary}"
    );

    let mut tx =
        Transaction::from_bytes(&hex::decode(v["unsigned_tx_hex"].as_str().unwrap()).unwrap())
            .unwrap();
    assert!(tx.signature.is_none(), "the draft carries no signature");
    assert_eq!(tx.nonce, NONCE);
    assert_eq!(
        tx.outputs[0].amount.to_string(),
        "1500000000000000000000000"
    );
    tx.sign(
        &generate_signing_key().unwrap(),
        &ChainTag::from_genesis(GENESIS),
    )
    .unwrap();
    tx.verify(&ChainTag::from_genesis(GENESIS))
        .expect("the wallet's signature over the draft verifies");
}

#[test]
fn draft_transfer_refuses_malformed_amounts_and_addresses_before_asking_the_node() {
    let (addr, seen) = mock_node(None);
    let to = hex::encode([1u8; 32]);
    let bad_amounts = [
        "1.0000000000000000000000001",
        "0",
        "0.0",
        "-1",
        "1e3",
        "",
        "1.2.3",
        ".5",
        "1,5",
        &"9".repeat(90),
    ];
    let mut requests: Vec<Value> = bad_amounts
        .iter()
        .enumerate()
        .map(|(i, a)| {
            tool(
                i as u64 + 1,
                "draft_transfer",
                &json!({"from": maya1([5; 32]), "to": to, "amount": a}),
            )
        })
        .collect();
    requests.push(tool(
        99,
        "draft_transfer",
        &json!({"from": "maya1nope", "to": to, "amount": "1"}),
    ));
    requests.push(tool(
        100,
        "draft_transfer",
        &json!({"from": maya1([5; 32]), "to": "abcd", "amount": "1"}),
    ));
    for reply in session(Some(&addr), &requests) {
        let (t, err) = text(&reply);
        assert!(err, "accepted a bad input: {t}");
    }
    assert!(
        seen.lock().unwrap().is_empty(),
        "bad input never reaches the node"
    );
}

#[test]
fn draft_transfer_says_when_the_balance_is_short_without_refusing() {
    let (addr, _) = mock_node(None);
    let v = ok_json(
        &session(
            Some(&addr),
            &[tool(
                1,
                "draft_transfer",
                &json!({"from": maya1([5; 32]), "to": hex::encode([1u8; 32]), "amount": "11"}),
            )],
        )[0],
    );
    assert_eq!(v["sufficient_balance"], false);
    assert!(v["summary"].as_str().unwrap().contains("NOT enough"));
}

#[test]
fn node_errors_and_a_missing_node_are_reported_not_hidden() {
    let (addr, _) = mock_node(Some("get_fee_info"));
    let (t, err) = text(&session(Some(&addr), &[tool(1, "estimate_fee", &json!({}))])[0]);
    assert!(err && t.contains("node is busy"), "{t}");
    let (t, err) = text(&session(None, &[tool(1, "get_height", &json!({}))])[0]);
    assert!(err && t.contains("no node configured"), "{t}");
}

#[test]
fn an_oversized_request_is_refused_and_the_next_one_still_works() {
    let (addr, _) = mock_node(None);
    let big = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "decode_transaction", "arguments": {"hex": "a".repeat(300_000)}}});
    let replies = session(Some(&addr), &[big, tool(2, "get_height", &json!({}))]);
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[0]["error"]["code"], -32600);
    assert!(!text(&replies[1]).1);
}

#[test]
fn decode_transaction_uses_the_node_decoder_and_explains_a_malformed_frame() {
    let replies = session(
        None,
        &[
            tool(
                1,
                "decode_transaction",
                &json!({"hex": vector("signed-transfer")}),
            ),
            tool(
                2,
                "decode_transaction",
                &json!({"hex": vector("trailing-byte")}),
            ),
        ],
    );
    let v = ok_json(&replies[0]);
    assert_eq!(v["signed"], true);
    assert!(v["txid"].as_str().is_some());
    let (t, err) = text(&replies[1]);
    assert!(err && t.contains("trailing"), "{t}");
}

#[test]
fn review_intent_flags_a_claim_mismatch_and_an_unlimited_approval() {
    let a = |b: u8| hex::encode([b; 32]);
    let reply = &session(
        None,
        &[tool(
            1,
            "review_intent",
            &json!({"claimed": [{"type": "call", "contract": a(1), "method": "claimAirdrop"}],
                    "simulated": [{"type": "approve", "token": a(2), "spender": a(3), "amount": "79228162514264337593543950336"}]}),
        )],
    )[0];
    let (t, err) = text(reply);
    assert!(
        !err && t.contains("ClaimMismatch") && t.contains("UnlimitedApproval"),
        "{t}"
    );
}

#[test]
fn explain_error_gives_the_spec_rule() {
    let (t, err) = text(
        &session(
            None,
            &[tool(1, "explain_error", &json!({"kind": "InvalidNonce"}))],
        )[0],
    );
    assert!(!err && t.contains("STF-1"), "{t}");
}
