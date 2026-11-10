//! The client against a mock gateway: a std TcpListener that answers from a
//! closure, so each test sees every request and can count them. No mock crate
//! is added to the lock.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use maya2c_sdk::{Client, Error};

const ADDRESS_HEX: &str = "06a9e5fc36d639816b77a9ab6619575df857f67503a8e33d05256a124e47d7cd";
const ADDRESS_TEXT: &str = "maya1qq657tlpk6cucz6mh4x4kvx2hthu90an4qw5wx0g9y44pynj86lxsgd736f";

struct Req {
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Req {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

struct Canned {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
    delay: Duration,
}

impl Canned {
    fn json(status: u16, body: &str) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.to_string(),
            delay: Duration::ZERO,
        }
    }

    fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    fn delayed(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

/// Starts a mock gateway. Returns its base URL.
fn serve<F>(handler: F) -> String
where
    F: Fn(&Req) -> Canned + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handler = Arc::new(handler);
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let handler = Arc::clone(&handler);
            thread::spawn(move || answer(stream, handler.as_ref()));
        }
    });
    format!("http://{addr}")
}

fn answer(mut stream: TcpStream, handler: &dyn Fn(&Req) -> Canned) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
        return;
    }
    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("")
        .to_string();
    let mut headers = Vec::new();
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_ascii_lowercase();
            let value = v.trim().to_string();
            if key == "content-length" {
                length = value.parse().unwrap_or(0);
            }
            headers.push((key, value));
        }
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).unwrap();
    let req = Req {
        path,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    };
    let canned = handler(&req);
    thread::sleep(canned.delay);
    let mut out = format!(
        "HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        canned.status,
        canned.body.len()
    );
    for (k, v) in &canned.headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str("\r\n");
    out.push_str(&canned.body);
    let _ = stream.write_all(out.as_bytes());
}

fn counter() -> Arc<AtomicUsize> {
    Arc::new(AtomicUsize::new(0))
}

#[test]
fn a_429_is_retried_once_and_the_second_answer_is_returned() {
    let hits = counter();
    let seen = Arc::clone(&hits);
    let base = serve(move |_| {
        if seen.fetch_add(1, Ordering::SeqCst) == 0 {
            Canned::json(429, r#"{"error":"slow down"}"#).with_header("Retry-After", "0")
        } else {
            Canned::json(200, r#"{"status":"ok"}"#)
        }
    });
    let health = Client::new(&base).unwrap().health().unwrap();
    assert_eq!(health.status, "ok");
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}

#[test]
fn a_503_is_retried_too() {
    let hits = counter();
    let seen = Arc::clone(&hits);
    let base = serve(move |_| {
        if seen.fetch_add(1, Ordering::SeqCst) == 0 {
            Canned::json(503, r#"{"error":"busy"}"#)
        } else {
            Canned::json(200, r#"{"status":"ok"}"#)
        }
    });
    Client::new(&base).unwrap().health().unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}

#[test]
fn a_get_gives_up_after_three_retries_with_rate_limited() {
    let hits = counter();
    let seen = Arc::clone(&hits);
    let base = serve(move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
        Canned::json(429, r#"{"error":"slow down"}"#).with_header("Retry-After", "0")
    });
    let err = Client::new(&base).unwrap().supply().unwrap_err();
    assert!(
        matches!(err, Error::RateLimited(ref e) if e.status == 429),
        "{err:?}"
    );
    assert_eq!(
        hits.load(Ordering::SeqCst),
        4,
        "one attempt plus three retries"
    );
}

#[test]
fn a_post_is_never_retried_on_429() {
    let hits = counter();
    let seen = Arc::clone(&hits);
    let base = serve(move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
        Canned::json(429, r#"{"error":"slow down"}"#).with_header("Retry-After", "0")
    });
    let err = Client::new(&base)
        .unwrap()
        .send_raw_transaction("aa")
        .unwrap_err();
    assert!(matches!(err, Error::RateLimited(_)), "{err:?}");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[test]
fn a_post_is_never_retried_on_503() {
    let hits = counter();
    let seen = Arc::clone(&hits);
    let base = serve(move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
        Canned::json(503, r#"{"error":"busy"}"#)
    });
    let err = Client::new(&base)
        .unwrap()
        .send_sealed_transaction("aa")
        .unwrap_err();
    assert!(
        matches!(err, Error::Api(ref e) if e.status == 503),
        "{err:?}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[test]
fn a_retry_after_longer_than_the_cap_fails_at_once() {
    let hits = counter();
    let seen = Arc::clone(&hits);
    let base = serve(move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
        Canned::json(429, r#"{"error":"slow down"}"#).with_header("Retry-After", "3600")
    });
    let started = Instant::now();
    let err = Client::new(&base).unwrap().fee_info().unwrap_err();
    assert!(
        matches!(err, Error::RateLimited(ref e) if e.retry_after == Some(Duration::from_secs(3600)))
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_balance_above_two_to_the_sixty_fourth_is_exact() {
    // 2^200, far past u64 and past a JSON number's 2^53.
    let big = "1606938044258990275541962092341162602522202993782792835301376";
    let base = serve(move |req| {
        assert_eq!(req.path, format!("/v1/accounts/{ADDRESS_HEX}"));
        Canned::json(200, &format!(r#"{{"balance":"{big}","nonce":3}}"#))
    });
    let balance = Client::new(&base).unwrap().balance(ADDRESS_TEXT).unwrap();
    assert_eq!(balance.balance.to_string(), big);
    assert_eq!(balance.nonce, 3);
}

#[test]
fn a_bad_address_is_refused_before_any_request() {
    let hits = counter();
    let seen = Arc::clone(&hits);
    let base = serve(move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
        Canned::json(200, "{}")
    });
    let client = Client::new(&base).unwrap();
    let typo = ADDRESS_TEXT.replace("gd736f", "gd736g");
    assert!(matches!(
        client.balance(&typo),
        Err(Error::InvalidAddress(_))
    ));
    assert!(matches!(
        client.balance("abcd"),
        Err(Error::InvalidAddress(_))
    ));
    assert!(matches!(
        client.balance(&"z".repeat(64)),
        Err(Error::InvalidAddress(_))
    ));
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[test]
fn the_user_agent_names_the_sdk_and_its_version() {
    let base = serve(|req| {
        let agent = req.header("user-agent").unwrap_or("").to_string();
        assert_eq!(
            agent,
            format!("maya2c-sdk-rust/{}", env!("CARGO_PKG_VERSION"))
        );
        Canned::json(200, r#"{"status":"ok"}"#)
    });
    Client::new(&base).unwrap().health().unwrap();
}

#[test]
fn a_gateway_error_body_becomes_a_typed_api_error() {
    let base = serve(|_| Canned::json(400, r#"{"error":"raw is empty"}"#));
    match Client::new(&base)
        .unwrap()
        .send_raw_transaction("")
        .unwrap_err()
    {
        Error::Api(e) => {
            assert_eq!(e.status, 400);
            assert_eq!(e.detail, "raw is empty");
            assert_eq!(e.code, "bad_request");
            assert_eq!(e.retry_after, None);
        }
        other => panic!("expected Api, got {other:?}"),
    }
}

#[test]
fn a_post_body_carries_the_raw_hex_under_raw() {
    let base = serve(|req| {
        assert_eq!(req.body, r#"{"raw":"deadbeef"}"#);
        Canned::json(200, r#"{"hash":"ab"}"#)
    });
    assert_eq!(
        Client::new(&base)
            .unwrap()
            .send_raw_transaction("deadbeef")
            .unwrap(),
        "ab"
    );
}

#[test]
fn a_sealed_submission_posts_the_ciphertext() {
    let base = serve(|req| {
        assert_eq!(req.path, "/v1/sealed");
        assert_eq!(req.body, r#"{"ciphertext":"c0ffee"}"#);
        Canned::json(200, r#"{"id":"f00d","bytes":3}"#)
    });
    let accepted = Client::new(&base)
        .unwrap()
        .send_sealed_transaction("c0ffee")
        .unwrap();
    assert_eq!((accepted.id.as_str(), accepted.bytes), ("f00d", 3));
}

#[test]
fn a_slow_gateway_hits_the_client_timeout() {
    let base = serve(|_| Canned::json(200, r#"{"status":"ok"}"#).delayed(Duration::from_secs(3)));
    let client = Client::with_timeout(&base, Duration::from_millis(200)).unwrap();
    assert!(matches!(client.health(), Err(Error::Timeout)));
}

#[test]
fn an_unreachable_gateway_is_a_network_error() {
    // Nothing listens on port 1 of loopback.
    let client = Client::with_timeout("http://127.0.0.1:1", Duration::from_secs(2)).unwrap();
    assert!(matches!(client.health(), Err(Error::Network(_))));
}

#[test]
fn a_malformed_success_body_is_a_decode_error() {
    let base = serve(|_| Canned::json(200, "not json"));
    assert!(matches!(
        Client::new(&base).unwrap().fee_info(),
        Err(Error::Decode(_))
    ));
}

#[test]
fn wait_for_transaction_returns_the_height_of_the_block_holding_it() {
    let chain_calls = counter();
    let seen = Arc::clone(&chain_calls);
    let txid = "AB".repeat(32);
    let base = serve(move |req| {
        if req.path == "/v1/chain" {
            // Height 5 on the first look, 6 after: block 6 arrives mid-wait.
            let n = seen.fetch_add(1, Ordering::SeqCst);
            let height = if n == 0 { 5 } else { 6 };
            Canned::json(200, &format!(r#"{{"genesis":"00","height":{height}}}"#))
        } else if req.path == "/v1/blocks/5" {
            Canned::json(200, r#"{"height":5,"transactions":[]}"#)
        } else if req.path == "/v1/blocks/6" {
            Canned::json(
                200,
                &format!(
                    r#"{{"height":6,"transactions":[{{"txid":"{}"}}]}}"#,
                    "ab".repeat(32)
                ),
            )
        } else {
            Canned::json(404, r#"{"error":"not found"}"#)
        }
    });
    let height = Client::new(&base)
        .unwrap()
        .wait_for_transaction(&txid, Duration::from_secs(10))
        .unwrap();
    assert_eq!(height, 6);
}

#[test]
fn wait_for_transaction_times_out_when_no_block_carries_it() {
    let base = serve(|req| {
        if req.path == "/v1/chain" {
            Canned::json(200, r#"{"genesis":"00","height":5}"#)
        } else {
            Canned::json(200, r#"{"height":5,"transactions":[]}"#)
        }
    });
    let started = Instant::now();
    let err = Client::new(&base)
        .unwrap()
        .wait_for_transaction("ab", Duration::from_millis(1200))
        .unwrap_err();
    assert!(matches!(err, Error::Timeout), "{err:?}");
    assert!(started.elapsed() < Duration::from_secs(5));
}
