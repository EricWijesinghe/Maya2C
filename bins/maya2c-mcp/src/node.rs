//! The one door to the node. Every request the server sends goes through
//! [`Node::call`], which accepts only the methods in [`READ_METHODS`] and
//! spends a token from a rate limiter first (ADR-084).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The only JSON-RPC methods this server sends. Each one reads state: none
/// signs, submits or changes anything. A test pins this list.
pub const READ_METHODS: [&str; 8] = [
    "get_chain_info",
    "get_tip_height",
    "get_balance",
    "get_balance_at_height",
    "get_block_by_height",
    "get_validators",
    "get_supply",
    "get_fee_info",
];

/// Sustained requests per second, the node's own default per-IP limit
/// (`crates/node/src/config.rs`), so this client never trips the node's.
const RATE_PER_SECOND: f64 = 50.0;
/// Burst size, the node's default (`crates/node/src/config.rs`).
const BURST: f64 = 100.0;
/// Connect and read timeout. A node that cannot answer in this time is an
/// error for the assistant to report, not a hang.
// shortcut: fixed 10 s, no retry; raise if a validator on a slow link needs it.
const RPC_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a paced call sleeps between token checks: one token's refill time.
const PACE_SLEEP: Duration = Duration::from_millis(20);
/// Largest node reply accepted, in bytes: the node's own RPC response limit
/// (`RPC_RESPONSE_LIMIT`, crates/node/src/rpc/server.rs), so a full reply fits.
const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;

/// Reads a reply until the node closes the connection. The deadline bounds the
/// whole reply, not each read, so a server that trickles bytes cannot hold it open.
fn read_capped(stream: &mut TcpStream, deadline: Instant, cap: usize) -> Result<Vec<u8>, String> {
    let mut reply = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let left = deadline
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
            .ok_or("node reply timed out")?;
        stream
            .set_read_timeout(Some(left))
            .map_err(|e| e.to_string())?;
        match stream.read(&mut chunk) {
            Ok(0) => return Ok(reply),
            Ok(n) => {
                if reply.len() + n > cap {
                    return Err("node reply exceeds the size cap".into());
                }
                reply.extend_from_slice(&chunk[..n]);
            }
            // A timed-out read loops back to the deadline check above.
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::Interrupted
                        | std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                ) => {}
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// A token bucket with an injectable clock, so the limit is testable.
pub struct Limiter {
    tokens: f64,
    last: Instant,
}

impl Limiter {
    pub fn new(now: Instant) -> Self {
        Self {
            tokens: BURST,
            last: now,
        }
    }

    /// Spends one token if one is available at `now`.
    pub fn admit(&mut self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + elapsed * RATE_PER_SECOND).min(BURST);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// A node reached over plain HTTP JSON-RPC at `--rpc HOST:PORT`.
pub struct Node {
    addr: Result<SocketAddr, String>,
    limiter: Limiter,
}

impl Node {
    /// `target` is the `--rpc` value, or `None` when the flag was not given.
    pub fn new(target: Option<&str>) -> Self {
        let addr = target
            .ok_or_else(|| "no node configured: start with --rpc HOST:PORT".to_string())
            .and_then(|t| t.parse::<SocketAddr>().map_err(|e| format!("--rpc: {e}")));
        Self {
            addr,
            limiter: Limiter::new(Instant::now()),
        }
    }

    /// Calls one read method and returns its `result`.
    ///
    /// # Errors
    ///
    /// Refuses methods outside [`READ_METHODS`], refuses when rate limited, and
    /// returns the node's own error message or any transport failure.
    pub fn call(&mut self, method: &str, params: &Value) -> Result<Value, String> {
        if !READ_METHODS.contains(&method) {
            return Err(format!("refused: {method} is not a read method"));
        }
        if !self.limiter.admit(Instant::now()) {
            return Err("rate limited: retry in a moment".into());
        }
        self.send(method, params)
    }

    /// [`Self::call`] for a long read sequence (a replay's block fetches):
    /// waits for a token instead of refusing, so the sequence holds the
    /// node's sustained rate rather than failing after the burst.
    ///
    /// # Errors
    ///
    /// As for [`Self::call`], except never "rate limited".
    pub fn call_paced(&mut self, method: &str, params: &Value) -> Result<Value, String> {
        if !READ_METHODS.contains(&method) {
            return Err(format!("refused: {method} is not a read method"));
        }
        while !self.limiter.admit(Instant::now()) {
            std::thread::sleep(PACE_SLEEP);
        }
        self.send(method, params)
    }

    fn send(&self, method: &str, params: &Value) -> Result<Value, String> {
        let addr = self.addr.clone()?;
        let body =
            json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
        let mut stream =
            TcpStream::connect_timeout(&addr, RPC_TIMEOUT).map_err(|e| e.to_string())?;
        stream
            .set_read_timeout(Some(RPC_TIMEOUT))
            .map_err(|e| e.to_string())?;
        stream
            .set_write_timeout(Some(RPC_TIMEOUT))
            .map_err(|e| e.to_string())?;
        let request = format!(
            "POST / HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream
            .write_all(request.as_bytes())
            .map_err(|e| e.to_string())?;
        let reply_bytes = read_capped(
            &mut stream,
            Instant::now() + RPC_TIMEOUT,
            MAX_RESPONSE_BYTES,
        )?;
        let response = String::from_utf8(reply_bytes).map_err(|e| e.to_string())?;
        let (_, json_part) = response
            .split_once("\r\n\r\n")
            .ok_or("malformed HTTP response")?;
        let reply: Value = serde_json::from_str(json_part).map_err(|e| e.to_string())?;
        if let Some(error) = reply.get("error") {
            let message = error["message"].as_str().unwrap_or("unknown node error");
            return Err(format!("node: {message}"));
        }
        Ok(reply["result"].clone())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn the_bucket_admits_a_burst_then_refills_at_the_node_rate() {
        let t0 = Instant::now();
        let mut limiter = Limiter::new(t0);
        let burst = (0..200).filter(|_| limiter.admit(t0)).count();
        assert_eq!(
            burst, 100,
            "a fresh client gets the node's burst, then no more"
        );
        let one_second_later = t0 + Duration::from_secs(1);
        let refilled = (0..200).filter(|_| limiter.admit(one_second_later)).count();
        assert_eq!(refilled, 50, "one second restores the sustained rate");
    }

    #[test]
    fn a_method_outside_the_read_list_never_leaves_the_process() {
        // Port 1 is never listening; the refusal must come before any connection.
        let mut node = Node::new(Some("127.0.0.1:1"));
        for method in ["send_raw_transaction", "submit_block", "htlc_get_lock"] {
            let Err(err) = node.call(method, &json!([])) else {
                panic!("{method} was sent to the node");
            };
            assert!(err.starts_with("refused"), "{method}: {err}");
        }
    }

    #[test]
    fn a_missing_or_malformed_target_is_an_error_not_a_default() {
        assert!(Node::new(None).call("get_supply", &json!([])).is_err());
        assert!(
            Node::new(Some("localhost:8545"))
                .call("get_supply", &json!([]))
                .is_err()
        );
    }

    /// A loopback connection whose server side runs `server` on accept.
    fn pair(server: impl FnOnce(TcpStream) + Send + 'static) -> TcpStream {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            server(stream);
        });
        TcpStream::connect(addr).unwrap()
    }

    #[test]
    fn a_reply_that_trickles_past_the_deadline_is_cut_off() {
        let mut client = pair(|mut s| {
            for _ in 0..200 {
                if s.write_all(b"x").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let deadline = Instant::now() + Duration::from_millis(200);
        let err = read_capped(&mut client, deadline, MAX_RESPONSE_BYTES).unwrap_err();
        assert!(err.contains("timed out"), "{err}");
    }

    #[test]
    fn a_reply_over_the_cap_is_refused() {
        let mut client = pair(|mut s| {
            let _ = s.write_all(&[b'x'; 4096]);
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        let err = read_capped(&mut client, deadline, 1024).unwrap_err();
        assert!(err.contains("size cap"), "{err}");
    }

    #[test]
    fn a_reply_under_the_cap_is_read_whole() {
        let mut client = pair(|mut s| {
            let _ = s.write_all(b"hello");
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        let body = read_capped(&mut client, deadline, 1024).unwrap();
        assert_eq!(body, b"hello");
    }
}
