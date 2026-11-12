//! MCP tools for agents (ADR-130, ADR-084). Read and draft only: nothing here
//! holds a key or submits a transaction. A registry is read over loopback HTTP,
//! and every card it returns is verified again before the model sees it.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::card::{MAX_TEXT_BYTES, SignedCard};
use crate::http::now;
use crate::payment::{SignedQuote, SignedReceipt, Transfer, draft_payment, verify_receipt};
use crate::wire;

const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// Largest registry reply accepted.
const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;
/// shortcut: loopback over plain HTTP only; a remote registry needs a TLS client, a later change.
const REFUSE_REGISTRY: &str = "registry must be http://127.0.0.1:<port> or http://localhost:<port>";

pub fn tools() -> Vec<Value> {
    vec![
        json!({"name": "agent_search", "description": "Agents offering a capability, from a local registry. Each card is verified before it is returned.",
               "inputSchema": {"type": "object", "properties": {
                   "registry": {"type": "string", "description": "http://127.0.0.1:<port> or http://localhost:<port>"},
                   "capability": {"type": "string", "description": "letters, digits and . _ : - only"}},
                   "required": ["registry", "capability"]}}),
        json!({"name": "agent_verify_card", "description": "Verify an agent card's signature and expiry. Returns the agent id.",
               "inputSchema": {"type": "object", "properties": {"card": {"type": "object"}}, "required": ["card"]}}),
        json!({"name": "agent_draft_payment", "description": "Check a signed quote and draft the transfer to pay it (to, amount, memo = quote hash). Signs nothing; the person's wallet signs.",
               "inputSchema": {"type": "object", "properties": {
                   "quote": {"type": "object"}, "payee_card": {"type": "object"},
                   "payer": {"type": "string", "description": "your agent id, 64 hex characters"}},
                   "required": ["quote", "payee_card", "payer"]}}),
        json!({"name": "agent_verify_receipt", "description": "Check that a payee-signed receipt describes a given transfer (txid, memo, to, amount, height).",
               "inputSchema": {"type": "object", "properties": {
                   "receipt": {"type": "object"}, "payee_card": {"type": "object"}, "transfer": {"type": "object"}},
                   "required": ["receipt", "payee_card", "transfer"]}}),
    ]
}

/// `None` when the name is not one of this surface's tools.
pub fn call(name: &str, args: &Value) -> Option<Result<Value, String>> {
    match name {
        "agent_search" => Some(search(args)),
        "agent_verify_card" => Some(verify_card(args)),
        "agent_draft_payment" => Some(draft(args)),
        "agent_verify_receipt" => Some(check_receipt(args)),
        _ => None,
    }
}

fn arg<T: DeserializeOwned>(args: &Value, key: &str) -> Result<T, String> {
    serde_json::from_value(args.get(key).cloned().unwrap_or(Value::Null))
        .map_err(|e| format!("{key}: {e}"))
}

fn verify_card(args: &Value) -> Result<Value, String> {
    let card: SignedCard = arg(args, "card")?;
    let id = card.verify(now()).map_err(|e| e.to_string())?;
    Ok(json!({"valid": true, "agent_id": hex::encode(id)}))
}

fn search(args: &Value) -> Result<Value, String> {
    let registry: String = arg(args, "registry")?;
    let capability: String = arg(args, "capability")?;
    let charset_ok = capability
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "._:-".contains(c));
    if capability.is_empty() || capability.len() > MAX_TEXT_BYTES || !charset_ok {
        return Err("capability: letters, digits and . _ : - only".to_owned());
    }
    let body = http_get_json(&registry, &format!("/v1/agents?capability={capability}"))?;
    let at = now();
    let agents: Vec<Value> = body
        .as_array()
        .ok_or_else(|| "registry reply is not a list".to_owned())?
        .iter()
        .filter_map(|v| serde_json::from_value::<SignedCard>(v.clone()).ok())
        .filter_map(|card| {
            card.verify(at)
                .ok()
                .map(|id| json!({"agent_id": hex::encode(id), "card": card}))
        })
        .collect();
    Ok(json!({"agents": agents}))
}

fn draft(args: &Value) -> Result<Value, String> {
    let quote: SignedQuote = arg(args, "quote")?;
    let payee: SignedCard = arg(args, "payee_card")?;
    let payer = wire::hex32_from_str(&arg::<String>(args, "payer")?)?;
    let at = now();
    payee.verify(at).map_err(|e| e.to_string())?;
    let draft =
        draft_payment(&quote, &payee.card.public_key, &payer, at).map_err(|e| e.to_string())?;
    serde_json::to_value(draft).map_err(|e| e.to_string())
}

fn check_receipt(args: &Value) -> Result<Value, String> {
    let receipt: SignedReceipt = arg(args, "receipt")?;
    let payee: SignedCard = arg(args, "payee_card")?;
    let transfer: Transfer = arg(args, "transfer")?;
    payee.verify(now()).map_err(|e| e.to_string())?;
    verify_receipt(&receipt, &payee.card.public_key, &transfer).map_err(|e| e.to_string())?;
    Ok(json!({"valid": true, "txid": hex::encode(receipt.receipt.txid)}))
}

/// Plain HTTP/1.1 GET to a loopback registry; returns the parsed JSON body.
fn http_get_json(base: &str, path: &str) -> Result<Value, String> {
    let host = base
        .strip_prefix("http://")
        .map(|h| h.trim_end_matches('/'))
        .ok_or(REFUSE_REGISTRY)?;
    // Exact name and a bare port: a prefix match let `127.0.0.1:80\r\nX: y` or
    // `localhost:80@elsewhere` through into the Host header. The connection
    // always goes to 127.0.0.1, so `localhost` never reaches a resolver.
    let port = host
        .split_once(':')
        .filter(|(name, _)| matches!(*name, "127.0.0.1" | "localhost"))
        .and_then(|(_, port)| port.parse::<u16>().ok())
        .filter(|port| *port != 0)
        .ok_or(REFUSE_REGISTRY)?;
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = TcpStream::connect_timeout(&addr, READ_TIMEOUT).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(READ_TIMEOUT))
        .map_err(|e| e.to_string())?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nAccept: application/json\r\n\r\n"
    )
    .map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    (&mut stream)
        .take(MAX_RESPONSE_BYTES)
        .read_to_end(&mut raw)
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&raw);
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or("registry reply has no body")?;
    if !head.starts_with("HTTP/1.1 200") {
        return Err("registry refused the request".to_owned());
    }
    serde_json::from_str(body).map_err(|e| e.to_string())
}
