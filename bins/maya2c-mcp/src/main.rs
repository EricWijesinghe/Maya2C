//! `maya2c-mcp` — a Model Context Protocol server for `Maya2C` (Master Prompt 24 §5).
//!
//! Speaks MCP's JSON-RPC 2.0 over stdio, one message per line: `initialize`,
//! `tools/list`, `tools/call`. Tools:
//!
//! - `decode_transaction` — the node's own decoder: fields, txid, sender.
//! - `review_intent` — `maya-clear-sign`'s pre-sign review of claimed versus
//!   simulated effects.
//! - `explain_error` — a node error kind in plain language, with the spec rule.
//! - `rpc_call` — a JSON-RPC call to a node given by `--rpc HOST:PORT`.
//!   **State-changing methods are refused unless the node is on loopback**,
//!   so an assistant can never write to a public network through this server;
//!   a person does that, with their own wallet.
//!
//! `maya2c-mcp --rpc 127.0.0.1:8545`

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};

use custom_l1_node::core::Transaction;
use maya_clear_sign::{Effect, review};
use serde_json::{Value, json};

const WRITES: [&str; 2] = ["send_raw_transaction", "submit_block"];

fn tools() -> Value {
    json!([
        {"name": "decode_transaction", "description": "Decode a hex-encoded Maya2C transaction with the node's decoder; returns version fields, txid and sender, or the decoder's error.",
         "inputSchema": {"type": "object", "properties": {"hex": {"type": "string"}}, "required": ["hex"]}},
        {"name": "review_intent", "description": "Clear-signing review: compare the effects an app claims with the effects a simulation shows; returns warnings, most severe first.",
         "inputSchema": {"type": "object", "properties": {"claimed": {"type": "array"}, "simulated": {"type": "array"}}, "required": ["claimed", "simulated"]}},
        {"name": "explain_error", "description": "Explain a node error kind (e.g. InvalidNonce) in plain language, with the spec rule it comes from.",
         "inputSchema": {"type": "object", "properties": {"kind": {"type": "string"}}, "required": ["kind"]}},
        {"name": "rpc_call", "description": "Call the configured node's JSON-RPC. Read methods only, unless the node is on loopback.",
         "inputSchema": {"type": "object", "properties": {"method": {"type": "string"}, "params": {"type": "array"}}, "required": ["method"]}}
    ])
}

fn decode(hex_str: &str) -> Result<Value, String> {
    let bytes = hex::decode(hex_str.trim()).map_err(|e| format!("not hex: {e}"))?;
    let tx = Transaction::from_bytes(&bytes).map_err(|e| e.to_string())?;
    let outputs: Vec<Value> = tx
        .outputs
        .iter()
        .map(|o| json!({"to": hex::encode(o.recipient), "amount": o.amount.to_string()}))
        .collect();
    Ok(
        json!({"version": bytes[0], "nonce": tx.nonce.to_string(), "outputs": outputs, "signed": tx.signature.is_some(),
              "txid": hex::encode(tx.txid()), "sender": hex::encode(tx.sender())}),
    )
}

fn addr(v: &Value) -> Result<[u8; 32], String> {
    let b = hex::decode(v.as_str().ok_or("address must be a hex string")?)
        .map_err(|e| e.to_string())?;
    b.try_into()
        .map_err(|_| "address must be 32 bytes".to_string())
}

fn amount(v: &Value) -> Result<u128, String> {
    v.as_str()
        .ok_or("amounts are decimal strings")?
        .parse()
        .map_err(|e| format!("{e}"))
}

/// `{"type": "transfer", "token": hex, "to": hex, "amount": "decimal"}` and the like.
fn effect(v: &Value) -> Result<Effect, String> {
    Ok(match v["type"].as_str().ok_or("effect needs a type")? {
        "transfer" => Effect::Transfer {
            token: addr(&v["token"])?,
            to: addr(&v["to"])?,
            amount: amount(&v["amount"])?,
        },
        "approve" => Effect::Approve {
            token: addr(&v["token"])?,
            spender: addr(&v["spender"])?,
            amount: amount(&v["amount"])?,
        },
        "approve_all" => Effect::ApproveAll {
            collection: addr(&v["collection"])?,
            operator: addr(&v["operator"])?,
        },
        "permit" => Effect::Permit {
            token: addr(&v["token"])?,
            spender: addr(&v["spender"])?,
            amount: amount(&v["amount"])?,
        },
        "upgrade" => Effect::Upgrade {
            contract: addr(&v["contract"])?,
        },
        "set_owner" => Effect::SetOwner {
            target: addr(&v["target"])?,
            owner: addr(&v["owner"])?,
        },
        "call" => Effect::Call {
            contract: addr(&v["contract"])?,
            method: v["method"].as_str().unwrap_or("").to_string(),
        },
        other => return Err(format!("unknown effect type {other}")),
    })
}

fn review_intent(args: &Value) -> Result<Value, String> {
    let list = |k: &str| -> Result<Vec<Effect>, String> {
        args[k]
            .as_array()
            .ok_or(format!("{k} must be an array"))?
            .iter()
            .map(effect)
            .collect()
    };
    let warnings = review(
        &list("claimed")?,
        &list("simulated")?,
        &maya_clear_sign::Context::default(),
    );
    Ok(json!(
        warnings
            .iter()
            .map(|w| format!("{w:?}"))
            .collect::<Vec<_>>()
    ))
}

fn explain(kind: &str) -> Result<Value, String> {
    let (text, rule) = match kind {
        "InvalidNonce" => (
            "The transaction's nonce is not the sender's next nonce: it was already used (a replay) or one is missing before it. Resend with the nonce `get_balance` reports.",
            "STF-1",
        ),
        "InsufficientBalance" => (
            "The sender's balance is below the sum of the outputs.",
            "STF-3",
        ),
        "BalanceOverflow" => (
            "An amount would exceed the largest representable balance (2^64 - 1).",
            "STF-2, STF-4, STF-5",
        ),
        "BadSignature" | "SignatureVerification" | "HashSignatureVerification" => (
            "One of the two signatures (ML-DSA-65 or SLH-DSA) does not verify; both must.",
            "TX-1",
        ),
        "UpgradeRequired" => (
            "The node reached a protocol version it does not implement. Install a newer binary; it resumes where it stopped.",
            "spec §4, ADR-020",
        ),
        "Decode" => (
            "The bytes are not a well-formed transaction frame: wrong version, bad length, or trailing bytes.",
            "ENC-1 … ENC-7",
        ),
        other => return Err(format!("unknown error kind {other}")),
    };
    Ok(json!({"explanation": text, "rule": rule}))
}

fn rpc(target: Option<&str>, args: &Value) -> Result<Value, String> {
    let target = target.ok_or("no node configured: start with --rpc HOST:PORT")?;
    let method = args["method"].as_str().ok_or("method is required")?;
    let addr: SocketAddr = target.parse().map_err(|e| format!("--rpc: {e}"))?;
    if WRITES.contains(&method) && !addr.ip().is_loopback() {
        return Err(format!(
            "refused: {method} changes state and {target} is not a local node; a person must do this with their own wallet"
        ));
    }
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": args.get("params").cloned().unwrap_or(json!([]))}).to_string();
    let mut s = TcpStream::connect(addr).map_err(|e| e.to_string())?;
    let request = format!(
        "POST / HTTP/1.1\r\nHost: {target}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(request.as_bytes()).map_err(|e| e.to_string())?;
    let mut resp = String::new();
    s.read_to_string(&mut resp).map_err(|e| e.to_string())?;
    let json_part = resp
        .split("\r\n\r\n")
        .nth(1)
        .ok_or("malformed HTTP response")?;
    serde_json::from_str(json_part).map_err(|e| e.to_string())
}

fn call_tool(name: &str, args: &Value, target: Option<&str>) -> Result<Value, String> {
    match name {
        "decode_transaction" => decode(args["hex"].as_str().ok_or("hex is required")?),
        "review_intent" => review_intent(args),
        "explain_error" => explain(args["kind"].as_str().ok_or("kind is required")?),
        "rpc_call" => rpc(target, args),
        other => Err(format!("unknown tool {other}")),
    }
}

fn handle(msg: &Value, target: Option<&str>) -> Option<Value> {
    let id = msg.get("id")?.clone();
    let result = match msg["method"].as_str().unwrap_or("") {
        "initialize" => {
            json!({"protocolVersion": "2025-06-18", "serverInfo": {"name": "maya2c-mcp", "version": env!("CARGO_PKG_VERSION")}, "capabilities": {"tools": {}}})
        }
        "tools/list" => json!({"tools": tools()}),
        "tools/call" => match call_tool(
            msg["params"]["name"].as_str().unwrap_or(""),
            &msg["params"]["arguments"],
            target,
        ) {
            Ok(v) => {
                json!({"content": [{"type": "text", "text": v.to_string()}], "isError": false})
            }
            Err(e) => json!({"content": [{"type": "text", "text": e}], "isError": true}),
        },
        other => {
            return Some(
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("method not found: {other}")}}),
            );
        }
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let target = args
        .iter()
        .position(|a| a == "--rpc")
        .and_then(|i| args.get(i + 1))
        .map(String::as_str);
    let stdout = std::io::stdout();
    for line in BufReader::new(std::io::stdin())
        .lines()
        .map_while(Result::ok)
    {
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(reply) = handle(&msg, target) {
            let mut out = stdout.lock();
            let _ = writeln!(out, "{reply}");
            let _ = out.flush();
        }
    }
}
