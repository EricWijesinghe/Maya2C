//! The read-only debugging tools (v3 P6) against a real dev chain.
//!
//! The fixture is the one `maya2c fork` runs: a dev genesis (the shape
//! `maya2c dev` writes), the node's own `Chain` making blocks from transfers,
//! and the node's own JSON-RPC server in front of it. The MCP binary is then
//! started the way an assistant starts it, over stdio.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use custom_l1_node::consensus::Chain;
use custom_l1_node::core::payload::{ContractCall, TxKind};
use custom_l1_node::core::{ChainTag, Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::validator_keys;
use custom_l1_node::genesis::GenesisConfig;
use custom_l1_node::network::Mempool;
use custom_l1_node::rpc::{RpcContext, RpcServer};
use custom_l1_node::state::StateDB;
use primitive_types::U256;
use serde_json::{Value, json};

const BOB: [u8; 32] = [0x55; 32];
const GAS: u64 = 10_000_000;

fn alice() -> HybridSigningKey {
    signing_key_from_seed(&[0x31; 32]).unwrap()
}

struct Devnet {
    rpc: RpcServer,
    genesis: PathBuf,
    tag: ChainTag,
    _runtime: tokio::runtime::Runtime,
}

fn transfer(tag: &ChainTag, amount: u64, nonce: u64) -> Transaction {
    let outputs = vec![TxOutput {
        amount: U256::from(amount),
        recipient: BOB,
    }];
    let mut tx = Transaction::new(vec![], outputs, nonce);
    tx.sign(&alice(), tag).unwrap();
    tx
}

/// Genesis, then block 1 (777 to Bob) and block 2 (100 more), served on RPC.
fn devnet(dir: &Path) -> Devnet {
    let validator = validator_keys::signing_key_from_seed(&[1; 32]).unwrap();
    let genesis = json!({
        "chain_id": "maya2c-dev", "timestamp": 1_000_000, "difficulty_bits": 0, "pow_limit_bits": 0,
        "allocations": [{"address": hex::encode(alice().address()), "balance": 1_000_000_000u64}],
        "bft": {"validators": [hex::encode(validator.verifying_key().to_bytes())], "anchor_timeout_ms": 500}
    });
    let path = dir.join("genesis.json");
    std::fs::write(&path, genesis.to_string()).unwrap();
    let config = GenesisConfig::from_json(&genesis.to_string()).unwrap();
    let state = Arc::new(StateDB::open(dir.join("node")).unwrap());
    config.seed_state(&state).unwrap();
    let block0 = config.genesis_block().unwrap();
    let tag = ChainTag::from_genesis(block0.header.id());
    let mut chain =
        Chain::open(Arc::clone(&state), block0, config.chain_config().unwrap()).unwrap();
    for (h, amount) in [(1u64, 777u64), (2, 100)] {
        let block = chain
            .candidate_block(1_000_000 + h, vec![transfer(&tag, amount, h - 1)])
            .unwrap();
        chain.insert_block(block).unwrap();
    }
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let context = RpcContext::new(Arc::new(Mutex::new(chain)), Mempool::new(state));
    let rpc = runtime
        .block_on(custom_l1_node::rpc::serve(
            "127.0.0.1:0".parse().unwrap(),
            context,
        ))
        .unwrap();
    Devnet {
        rpc,
        genesis: path,
        tag,
        _runtime: runtime,
    }
}

fn session(net: &Devnet, calls: &[(&str, Value)]) -> Vec<(Value, bool)> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_maya2c-mcp"))
        .args(["--rpc", &net.rpc.address.to_string(), "--genesis"])
        .arg(&net.genesis)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for (id, (name, args)) in calls.iter().enumerate() {
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": args}});
        writeln!(stdin, "{msg}").unwrap();
    }
    drop(stdin);
    let replies = BufReader::new(child.stdout.take().unwrap())
        .lines()
        .map(|l| {
            let reply: Value = serde_json::from_str(&l.unwrap()).unwrap();
            let text = reply["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .to_string();
            let is_error = reply["result"]["isError"].as_bool().unwrap();
            (
                serde_json::from_str(&text).unwrap_or(Value::String(text)),
                is_error,
            )
        })
        .collect();
    child.wait().unwrap();
    replies
}

fn nft_game() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target-contracts/wasm32-unknown-unknown/release/nft_game.wasm");
    hex::encode(std::fs::read(path).expect("the tracked nft_game.wasm"))
}

#[test]
fn replay_returns_the_blocks_state_diff() {
    let dir = tempfile::tempdir().unwrap();
    let net = devnet(dir.path());
    let txid = hex::encode(transfer(&net.tag, 100, 1).txid());
    let replies = session(
        &net,
        &[
            ("replay_block", json!({"height": 2})),
            ("replay_block", json!({"height": 2, "txid": txid})),
            ("replay_block", json!({"height": 1, "txid": txid})),
        ],
    );
    let (block, err) = &replies[0];
    assert!(!err, "{block}");
    assert_eq!(block["reproduced"], true);
    let bob = block["balance_changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["address"] == hex::encode(BOB))
        .expect("Bob's balance moved");
    assert_eq!(
        (bob["before"].as_str(), bob["after"].as_str()),
        (Some("777"), Some("877"))
    );
    assert!(!replies[1].1 && replies[1].0["transactions"][0] == txid.as_str());
    assert!(replies[2].1, "the tx is not in block 1: {}", replies[2].0);
}

#[test]
fn compare_account_between_two_heights() {
    let dir = tempfile::tempdir().unwrap();
    let net = devnet(dir.path());
    let replies = session(
        &net,
        &[(
            "compare_account",
            json!({"address": hex::encode(BOB), "from": 1, "to": 2}),
        )],
    );
    let (out, err) = &replies[0];
    assert!(!err, "{out}");
    assert_eq!(out["from"]["balance"], "777");
    assert_eq!(out["to"]["balance"], "877");
    assert_eq!(out["change"], "+100");
}

#[test]
fn trace_a_contract_call_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let net = devnet(dir.path());
    let admin = alice().address();
    let mut tx = Transaction::new(vec![], vec![], 2);
    tx.kind = TxKind::CallContract(ContractCall {
        contract: [77; 32],
        input: [&[0u8][..], &admin[..]].concat(),
        gas_limit: GAS,
    });
    tx.sign(&alice(), &net.tag).unwrap();
    let transfer_hex = hex::encode(transfer(&net.tag, 1, 2).to_bytes());
    let replies = session(
        &net,
        &[
            (
                "trace_transaction",
                json!({"wasm": nft_game(), "tx": hex::encode(tx.to_bytes()), "height": 10}),
            ),
            (
                "trace_transaction",
                json!({"wasm": nft_game(), "tx": transfer_hex}),
            ),
        ],
    );
    let (trace, err) = &replies[0];
    assert!(!err, "{trace}");
    assert!(
        trace["outcome"].as_str().unwrap().starts_with("ok"),
        "{trace}"
    );
    let steps = trace["steps"].as_array().unwrap();
    assert!(
        steps.iter().any(|s| s.as_str().unwrap().contains("write")),
        "{trace}"
    );
    assert!(!trace["storage_after"].as_array().unwrap().is_empty());
    assert!(replies[1].1, "a transfer has no execution steps");
}

/// One JSON-RPC request straight to the devnet node, bypassing the MCP server.
fn post(net: &Devnet, body: &Value) -> Value {
    use std::io::Read;
    let body = body.to_string();
    let mut s = std::net::TcpStream::connect(net.rpc.address).unwrap();
    write!(
        s,
        "POST / HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        net.rpc.address,
        body.len()
    )
    .unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    serde_json::from_str(resp.split_once("\r\n\r\n").unwrap().1).unwrap()
}

#[test]
fn a_real_refusal_from_the_node_is_explained() {
    let dir = tempfile::tempdir().unwrap();
    let net = devnet(dir.path());
    // Nonce 0 was spent in block 1: the node refuses it as a replay.
    let replayed = hex::encode(transfer(&net.tag, 5, 0).to_bytes());
    // The server has no submit tool (ADR-084), so the test submits directly.
    let refusal = post(
        &net,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "send_raw_transaction", "params": [replayed]}),
    );
    let message = refusal["error"]["message"]
        .as_str()
        .expect("the node refused")
        .to_string();
    let replies = session(&net, &[("explain_refusal", json!({"message": message}))]);
    let (out, err) = &replies[0];
    assert!(!err, "{out}");
    let kind = out["cause"]["kind"].as_str().or(out["kind"].as_str());
    assert_eq!(kind, Some("InvalidNonce"), "{message} -> {out}");
}

#[test]
fn a_replay_above_the_cap_is_refused_before_any_fetch() {
    let dir = tempfile::tempdir().unwrap();
    let genesis = dir.path().join("genesis.json");
    std::fs::write(&genesis, "{}").unwrap();
    // Port 1 never listens: a refusal that mentions the cap came before any fetch.
    let mut child = Command::new(env!("CARGO_BIN_EXE_maya2c-mcp"))
        .args(["--rpc", "127.0.0.1:1", "--genesis"])
        .arg(&genesis)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let msg = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "replay_block", "arguments": {"height": 10_001}}});
    writeln!(child.stdin.take().unwrap(), "{msg}").unwrap();
    let line = BufReader::new(child.stdout.take().unwrap())
        .lines()
        .next()
        .unwrap()
        .unwrap();
    child.wait().unwrap();
    let reply: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(reply["result"]["isError"], true);
    let text = reply["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("capped at height 10000"), "{text}");
}

#[test]
fn replay_fetches_only_through_the_allowlisted_client() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/debug_tools.rs"))
        .unwrap();
    // Each of these opens its own connection, past `Node::call`'s allowlist
    // and rate limiter.
    for bypass in [
        "RpcBootstrapSource",
        "fork::replay(",
        "fork::copy(",
        "replay(&",
        "TcpStream",
    ] {
        assert!(!src.contains(bypass), "debug_tools.rs uses {bypass}");
    }
    assert!(src.contains("call_paced"));
}

#[test]
fn no_debugging_tool_reaches_a_write_method() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/debug_tools.rs"))
        .unwrap();
    for write in ["send_raw_transaction", "submit_block"] {
        assert!(!src.contains(write), "debug_tools.rs names {write}");
    }
}
