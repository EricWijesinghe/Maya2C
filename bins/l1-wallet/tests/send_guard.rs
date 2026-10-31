//! Red-team runs of the `l1-wallet send` binary against a fake node.
//!
//! The fake node answers the three RPC methods `send` reads and records every
//! method it is called with. A refusal must leave `send_raw_transaction`
//! uncalled, and a refused run must not print a signed transaction.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex, OnceLock};

use custom_l1_node::crypto::hybrid::generate_signing_key;
use custom_l1_node::state::fees::FEE_COLLECTOR;
use l1_wallet::keystore;
use serde_json::{Value, json};

const PASSWORD: &str = "guard-test-password";
const GENESIS: &str = "aa11bb22cc33dd44ee55ff6600778899aa11bb22cc33dd44ee55ff6600778899";
const OTHER_GENESIS: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const RECIPIENT: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const OTHER_COLLECTOR: &str = "2222222222222222222222222222222222222222222222222222222222222222";

/// One keystore for every test: key generation is the slow step.
fn keystore_path() -> &'static PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("l1w-send-guard-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create the test directory");
        let path = dir.join("wallet.key");
        let key = generate_signing_key().expect("generate a test key");
        let blob = keystore::encrypt(&key, PASSWORD).expect("encrypt the test key");
        std::fs::write(&path, blob).expect("write the test keystore");
        path
    })
}

/// Reads one HTTP request and returns its body.
fn read_body(stream: &TcpStream) -> String {
    let mut reader = BufReader::new(stream.try_clone().expect("clone the socket"));
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).expect("read a header");
        if line.is_empty() || line == "\r\n" {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse().expect("a numeric length");
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).expect("read the body");
    String::from_utf8(body).expect("UTF-8 body")
}

/// A fake node. Returns its URL and the list of RPC methods it was called with.
fn fake_node(genesis: &str, collector: &str, base_fee: &str) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the fake node");
    let url = format!("http://{}", listener.local_addr().expect("local address"));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&calls);
    let chain = json!({ "genesis": genesis, "chain_id": "maya-testnet-1", "height": 7 });
    let fees = json!({ "active": true, "base_fee": base_fee, "collector": collector });
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let request: Value = serde_json::from_str(&read_body(&stream)).unwrap_or(Value::Null);
            let method = request["method"].as_str().unwrap_or_default().to_string();
            let result = match method.as_str() {
                "get_chain_info" => chain.clone(),
                "get_fee_info" => fees.clone(),
                _ => json!({ "txid": "00", "accepted": true }),
            };
            seen.lock().expect("the call log").push(method);
            let reply = json!({ "jsonrpc": "2.0", "id": request["id"], "result": result });
            let body = reply.to_string();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (url, calls)
}

fn protocol_collector() -> String {
    hex::encode(FEE_COLLECTOR)
}

/// Runs `l1-wallet send` with stdin closed, so the child never sees a terminal.
fn run_send(rpc_url: &str, amount: &str, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_l1-wallet"))
        .arg("--keystore")
        .arg(keystore_path())
        .arg("--rpc-url")
        .arg(rpc_url)
        .args([
            "send", "--to", RECIPIENT, "--amount", amount, "--nonce", "0",
        ])
        .args(extra)
        .env("L1_WALLET_PASSWORD", PASSWORD)
        .stdin(Stdio::null())
        .output()
        .expect("run l1-wallet")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn broadcast_attempted(calls: &Arc<Mutex<Vec<String>>>) -> bool {
    calls
        .lock()
        .expect("the call log")
        .iter()
        .any(|m| m == "send_raw_transaction")
}

#[test]
fn an_unpinned_genesis_without_the_flag_is_refused_and_not_broadcast() {
    let (url, calls) = fake_node(GENESIS, &protocol_collector(), "1");
    let output = run_send(&url, "1000000000000000000000000000000", &[]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("not pinned"),
        "{}",
        stderr(&output)
    );
    assert!(!stdout(&output).contains("raw:"));
    assert!(!broadcast_attempted(&calls));
}

#[test]
fn a_genesis_flag_that_differs_from_the_node_is_refused() {
    let (url, calls) = fake_node(GENESIS, &protocol_collector(), "1");
    let output = run_send(
        &url,
        "1000000000000000000000000000000",
        &["--genesis", OTHER_GENESIS, "--yes"],
    );
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("refusing to sign"),
        "{}",
        stderr(&output)
    );
    assert!(!broadcast_attempted(&calls));
}

#[test]
fn a_collector_that_is_not_the_protocol_one_is_refused() {
    let (url, calls) = fake_node(GENESIS, OTHER_COLLECTOR, "1");
    let output = run_send(
        &url,
        "1000000000000000000000000000000",
        &["--genesis", GENESIS, "--yes"],
    );
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("not the protocol collector"),
        "{}",
        stderr(&output)
    );
    assert!(!broadcast_attempted(&calls));
}

#[test]
fn a_node_quote_above_the_cap_is_refused() {
    // Cap is 10^18 base units. A 10^14 base-fee quote on a transaction of about
    // 13 KB is 2 * 10^14 * 13,000 = 2.6 * 10^18, above it.
    let (url, calls) = fake_node(GENESIS, &protocol_collector(), "100000000000000");
    let output = run_send(&url, "1000", &["--genesis", GENESIS, "--yes"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("above the cap"),
        "{}",
        stderr(&output)
    );
    assert!(!broadcast_attempted(&calls));
}

#[test]
fn without_a_terminal_or_yes_nothing_is_signed() {
    let (url, calls) = fake_node(GENESIS, &protocol_collector(), "1");
    let output = run_send(
        &url,
        "1000000000000000000000000000000",
        &["--genesis", GENESIS],
    );
    assert!(!output.status.success());
    assert!(stderr(&output).contains("--yes"), "{}", stderr(&output));
    assert!(!stdout(&output).contains("raw:"));
    assert!(!broadcast_attempted(&calls));
}

#[test]
fn a_dust_transfer_signs_within_the_absolute_cap_and_warns() {
    // Amount 1 base unit: a fee of about 26,000 is far above a tenth of it, but
    // well within the absolute cap, so it is warned about, not refused.
    let (url, _calls) = fake_node(GENESIS, &protocol_collector(), "1");
    let output = run_send(
        &url,
        "1",
        &["--genesis", GENESIS, "--yes", "--no-broadcast"],
    );
    assert!(
        output.status.success(),
        "{}{}",
        stdout(&output),
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("more than a tenth"),
        "{}",
        stderr(&output)
    );
    assert!(stdout(&output).contains("raw:      "));
}

#[test]
fn yes_prints_the_description_and_signs_for_the_pinned_chain() {
    let (url, calls) = fake_node(GENESIS, &protocol_collector(), "1");
    let output = run_send(
        &url,
        "1000000000000000000000000000000",
        &["--genesis", GENESIS, "--yes", "--no-broadcast"],
    );
    let out = stdout(&output);
    assert!(output.status.success(), "{}{}", out, stderr(&output));
    assert!(out.contains(&format!("genesis:  {GENESIS}")), "{out}");
    assert!(out.contains("chain:    maya-testnet-1"), "{out}");
    assert!(out.contains(&format!("pay to: {RECIPIENT}")), "{out}");
    assert!(out.contains("fee:      "), "{out}");
    assert!(out.contains("raw:      "), "{out}");
    assert!(!broadcast_attempted(&calls));
}
