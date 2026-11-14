//! `maya2c-mcp` — a Model Context Protocol server for `Maya2C` (Master Prompt 24 §5, ADR-084).
//!
//! Speaks MCP's JSON-RPC 2.0 over stdio, one message per line: `initialize`,
//! `tools/list`, `tools/call`. The server holds no keys and has no tool that
//! signs or submits. It reads the chain through a fixed list of read-only
//! node methods, and drafts transfers for a person's own wallet to sign.
//! It also keeps an in-memory intent book (P-30, ADR-139) for intents a
//! maker's wallet has already signed.
//!
//! `maya2c-mcp --rpc 127.0.0.1:8545`

mod intents;
mod node;
mod tools;

use std::io::{BufRead, BufReader, Read, Write};

use node::Node;
use serde_json::{Value, json};

/// Largest request line accepted, in bytes. A signed transfer is about 14 KB
/// in hex, so this leaves room for a long batch of arguments.
// shortcut: 256 KiB is a generous guess, not measured; tighten after measuring a real request.
const MAX_REQUEST_BYTES: usize = 256 * 1024;

enum Line {
    End,
    Text(String),
    TooLong,
}

/// Reads one line without buffering more than [`MAX_REQUEST_BYTES`]. An
/// oversized line is drained to its end and reported, so the next request
/// still parses.
fn read_line(input: &mut impl BufRead) -> std::io::Result<Line> {
    let mut buf = Vec::new();
    let cap = u64::try_from(MAX_REQUEST_BYTES + 1).unwrap_or(u64::MAX);
    let read = Read::take(&mut *input, cap).read_until(b'\n', &mut buf)?;
    if read == 0 {
        return Ok(Line::End);
    }
    if buf.last() == Some(&b'\n') || buf.len() <= MAX_REQUEST_BYTES {
        return Ok(Line::Text(String::from_utf8_lossy(&buf).into_owned()));
    }
    let mut sink = Vec::new();
    loop {
        sink.clear();
        let got = Read::take(&mut *input, 4096).read_until(b'\n', &mut sink)?;
        if got == 0 || sink.last() == Some(&b'\n') {
            return Ok(Line::TooLong);
        }
    }
}

/// An intent tool if the name is one, otherwise a chain tool.
fn call_tool(msg: &Value, node: &mut Node, book: &mut intents::Intents) -> Result<Value, String> {
    let name = msg["params"]["name"].as_str().unwrap_or("");
    let args = &msg["params"]["arguments"];
    intents::call(book, node, name, args).unwrap_or_else(|| tools::call(node, name, args))
}

fn handle(msg: &Value, node: &mut Node, book: &mut intents::Intents) -> Option<Value> {
    let id = msg.get("id")?.clone();
    let result = match msg["method"].as_str().unwrap_or("") {
        "initialize" => {
            json!({"protocolVersion": "2025-06-18", "serverInfo": {"name": "maya2c-mcp", "version": env!("CARGO_PKG_VERSION")}, "capabilities": {"tools": {}}})
        }
        "tools/list" => {
            let mut all = tools::list();
            if let Some(list) = all.as_array_mut() {
                list.extend(intents::list());
            }
            json!({"tools": all})
        }
        "tools/call" => match call_tool(msg, node, book) {
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
    let mut node = Node::new(target);
    let mut book = intents::Intents::default();
    let stdout = std::io::stdout();
    let stdin = std::io::stdin();
    let mut input = BufReader::new(stdin.lock());
    loop {
        let reply = match read_line(&mut input) {
            Ok(Line::End) | Err(_) => break,
            Ok(Line::TooLong) => Some(
                json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32600, "message": "request exceeds the size cap"}}),
            ),
            Ok(Line::Text(text)) => match serde_json::from_str::<Value>(&text) {
                Ok(msg) => handle(&msg, &mut node, &mut book),
                Err(_) => None,
            },
        };
        if let Some(reply) = reply {
            let mut out = stdout.lock();
            let _ = writeln!(out, "{reply}");
            let _ = out.flush();
        }
    }
}
