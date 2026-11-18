//! A stand-in node for the end-to-end suite: answers the three JSON-RPC
//! reads the Send screen needs (`get_chain_info`, `get_fee_info`,
//! `get_balance`) on a local port, so a review can be built while the public
//! testnet is paused. It cannot accept a transaction: anything else is a
//! JSON-RPC "method not found", so a test that tried to broadcast would fail
//! loudly instead of pretending a transfer happened.
//!
//! The genesis is a fixed test value, never the mainnet one, so the wallet
//! classifies this node as a test network (`networks.rs`).

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};

use serde_json::{Value, json};

/// The stub's genesis id: 32 bytes of `0xe2`, a test network by construction.
pub const GENESIS: &str = "e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2";
/// The network name it reports.
pub const NETWORK: &str = "maya-e2e-stub";
/// The tip height it reports.
pub const HEIGHT: u64 = 4242;
/// Every account's balance, base units: 1,000 MAYA at 24 decimals (ADR-068).
pub const BALANCE: &str = "1000000000000000000000000000";

/// A running stub. The listener thread lives as long as the test process,
/// which is the suite's lifetime.
pub struct StubNode {
    url: String,
}

impl StubNode {
    /// Listens on a free local port.
    ///
    /// # Errors
    ///
    /// No local port could be bound.
    pub fn start() -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let url = format!("http://{}/rpc", listener.local_addr()?);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                std::thread::spawn(move || serve(stream));
            }
        });
        Ok(Self { url })
    }

    /// The URL to give the wallet as its node.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }
}

/// Answers requests on one keep-alive connection until the client closes it.
fn serve(stream: TcpStream) {
    let Ok(mut out) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(stream);
    while let Some(body) = read_request(&mut reader) {
        let reply = answer(&body).to_string();
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            reply.len()
        );
        if out.write_all(head.as_bytes()).is_err() || out.write_all(reply.as_bytes()).is_err() {
            return;
        }
    }
}

/// One HTTP request's body, or `None` at end of stream or on a malformed head.
fn read_request(reader: &mut impl BufRead) -> Option<Vec<u8>> {
    let mut length = 0usize;
    let mut started = false;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            if started {
                break;
            }
            continue;
        }
        started = true;
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().ok()?;
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).ok()?;
    Some(body)
}

/// The JSON-RPC reply to one request body.
#[must_use]
pub fn answer(body: &[u8]) -> Value {
    let Ok(request) = serde_json::from_slice::<Value>(body) else {
        return json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "parse error"}});
    };
    let id = request["id"].clone();
    let result = match request["method"].as_str().unwrap_or_default() {
        "get_chain_info" => json!({"genesis": GENESIS, "chain_id": NETWORK, "height": HEIGHT}),
        "get_fee_info" => json!({"active": false, "base_fee": "0", "collector": ""}),
        "get_balance" => json!({
            "address": request["params"][0].as_str().unwrap_or_default(),
            "balance": BALANCE,
            "nonce": 0,
        }),
        other => {
            return json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("the e2e stub does not serve {other}")}});
        }
    };
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn reads_are_answered_and_anything_else_is_refused() {
        let chain = answer(br#"{"jsonrpc":"2.0","id":1,"method":"get_chain_info","params":[]}"#);
        assert_eq!(chain["id"], 1);
        assert_eq!(chain["result"]["genesis"], GENESIS);
        let balance = answer(br#"{"jsonrpc":"2.0","id":2,"method":"get_balance","params":["ab"]}"#);
        assert_eq!(balance["result"]["balance"], BALANCE);
        assert_eq!(balance["result"]["address"], "ab");
        let send = answer(br#"{"jsonrpc":"2.0","id":3,"method":"send_raw_transaction"}"#);
        assert_eq!(send["error"]["code"], -32601, "a broadcast is never faked");
    }

    #[test]
    fn a_keep_alive_connection_serves_two_requests() {
        let node = StubNode::start().unwrap();
        let address = node
            .url()
            .trim_start_matches("http://")
            .trim_end_matches("/rpc");
        let mut stream = TcpStream::connect(address).unwrap();
        let body = r#"{"jsonrpc":"2.0","id":7,"method":"get_fee_info","params":[]}"#;
        let request = format!(
            "POST /rpc HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        for _ in 0..2 {
            stream.write_all(request.as_bytes()).unwrap();
            let reply = read_request(&mut reader).expect("a reply");
            let value: Value = serde_json::from_slice(&reply).unwrap();
            assert_eq!(value["result"]["active"], false);
        }
    }
}
