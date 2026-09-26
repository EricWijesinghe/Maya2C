//! The MCP server's task set (Master Prompt 24 §5): the binary is started as
//! an assistant would start it, over stdio, and driven through 10 tasks.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

/// A one-shot fake node: answers every request with `{"result": 42}`.
fn fake_node() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let mut s = s;
            // Read the whole request (headers, then Content-Length bytes)
            // before answering, so closing never discards unread input.
            let mut req = Vec::new();
            let mut buf = [0u8; 4096];
            while let Ok(n) = s.read(&mut buf) {
                if n == 0 {
                    break;
                }
                req.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&req);
                if let Some(end) = text.find("\r\n\r\n") {
                    let len = text
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if req.len() >= end + 4 + len {
                        break;
                    }
                }
            }
            let body = r#"{"jsonrpc":"2.0","id":1,"result":42}"#;
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
        }
    });
    addr
}

fn session(rpc: &str, requests: &[Value]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_maya2c-mcp"))
        .args(["--rpc", rpc])
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

fn text(reply: &Value) -> (String, bool) {
    (
        reply["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string(),
        reply["result"]["isError"].as_bool().unwrap(),
    )
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

#[test]
fn ten_assistant_tasks() {
    let a = |b: u8| hex::encode([b; 32]);
    let node = fake_node();
    let replies = session(
        &node,
        &[
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
            tool(
                3,
                "decode_transaction",
                &json!({"hex": vector("signed-transfer")}),
            ),
            tool(
                4,
                "decode_transaction",
                &json!({"hex": vector("trailing-byte")}),
            ),
            tool(
                5,
                "review_intent",
                &json!({"claimed": [{"type": "call", "contract": a(1), "method": "claimAirdrop"}],
                                        "simulated": [{"type": "approve", "token": a(2), "spender": a(3), "amount": "79228162514264337593543950336"}]}),
            ),
            tool(6, "explain_error", &json!({"kind": "InvalidNonce"})),
            tool(7, "rpc_call", &json!({"method": "get_tip_height"})),
            tool(
                8,
                "rpc_call",
                &json!({"method": "send_raw_transaction", "params": ["00"]}),
            ),
            tool(9, "no_such_tool", &json!({})),
            json!({"jsonrpc": "2.0", "id": 10, "method": "resources/list"}),
        ],
    );
    assert_eq!(replies.len(), 10);
    assert_eq!(replies[0]["result"]["serverInfo"]["name"], "maya2c-mcp");
    assert_eq!(replies[1]["result"]["tools"].as_array().unwrap().len(), 4);
    let (t, err) = text(&replies[2]);
    assert!(
        !err && t.contains("\"txid\"") && t.contains("\"signed\":true"),
        "{t}"
    );
    let (t, err) = text(&replies[3]);
    assert!(
        err && t.contains("trailing"),
        "a malformed frame is explained: {t}"
    );
    let (t, err) = text(&replies[4]);
    assert!(
        !err && t.contains("ClaimMismatch") && t.contains("UnlimitedApproval"),
        "{t}"
    );
    let (t, _) = text(&replies[5]);
    assert!(t.contains("STF-1"));
    let (t, err) = text(&replies[6]);
    assert!(!err && t.contains("42"), "read call reaches the node: {t}");
    let (t, err) = text(&replies[7]);
    assert!(
        !err && t.contains("42"),
        "a write to a loopback node is allowed and reaches it: {t}"
    );
    let (_, err) = text(&replies[8]);
    assert!(err);
    assert_eq!(replies[9]["error"]["code"], -32601);
    println!("MCP task set: 10/10");
}

#[test]
fn writes_to_a_public_node_are_refused() {
    // 203.0.113.0/24 is TEST-NET-3: never reachable, and the refusal comes before any connection.
    let replies = session(
        "203.0.113.7:8545",
        &[tool(
            1,
            "rpc_call",
            &json!({"method": "send_raw_transaction", "params": ["00"]}),
        )],
    );
    let (t, err) = text(&replies[0]);
    assert!(err && t.starts_with("refused"), "{t}");
}
