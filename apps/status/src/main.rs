//! `maya-status`: watches a node, derives uptime from block timestamps, and
//! serves a status page and `/api/status`.
//!
//! ```text
//! maya-status --node http://127.0.0.1:8545 --listen 127.0.0.1:3100 \
//!             [--threshold 60] [--ntfy https://ntfy.sh/<private-topic>]
//! ```
//!
//! On start it reads every block from genesis (the history is the chain's,
//! recomputed, never stored here), then follows the tip. With `--ntfy` it
//! posts one message when the chain stops for longer than the threshold and
//! one when it resumes.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::extract::State;
use axum::response::{Html, IntoResponse, Json};
use axum::routing::get;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use jsonrpsee::rpc_params;
use maya_status::Timeline;

const POLL: Duration = Duration::from_secs(2);

/// Headers per `get_headers` call: the node's own cap.
const BATCH: u64 = 2_000;

/// Blocks behind the tip still counted as caught up: one poll's worth.
const CAUGHT_UP: u64 = 5;

/// The timeline, and the node's tip, so a report is only given (and an
/// alert only sent) once the history has been read: mid-catch-up the
/// newest block seen is hours old and would read as a halt.
struct Shared {
    timeline: Mutex<Timeline>,
    tip: AtomicU64,
}

impl Shared {
    fn caught_up(&self) -> Option<u64> {
        let seen = self.timeline.lock().ok()?.height()?;
        let tip = self.tip.load(Ordering::Relaxed);
        (seen + CAUGHT_UP >= tip).then_some(seen)
    }
}

struct Args {
    node: String,
    listen: SocketAddr,
    threshold: u64,
    ntfy: Option<String>,
}

fn args() -> Result<Args, String> {
    let mut a = Args {
        node: "http://127.0.0.1:8545".into(),
        listen: "127.0.0.1:3100".parse().map_err(|e| format!("{e}"))?,
        threshold: 60,
        ntfy: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--node" => a.node = value()?,
            "--listen" => a.listen = value()?.parse().map_err(|e| format!("--listen: {e}"))?,
            "--threshold" => {
                a.threshold = value()?.parse().map_err(|e| format!("--threshold: {e}"))?;
            }
            "--ntfy" => a.ntfy = Some(value()?),
            "--help" | "-h" => {
                return Err(
                    "usage: maya-status --node URL --listen ADDR [--threshold SECS] [--ntfy URL]"
                        .into(),
                );
            }
            other => return Err(format!("unknown flag {other}")),
        }
    }
    Ok(a)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

async fn height(client: &HttpClient) -> Option<u64> {
    let info: serde_json::Value = client.request("get_chain_info", rpc_params![]).await.ok()?;
    info.get("height")?.as_u64()
}

/// Byte range of the timestamp in a serialized header: after the 32-byte
/// parent hash and 32-byte state root, little-endian `u64`.
const TIMESTAMP_AT: std::ops::Range<usize> = 64..72;

/// Timestamps of blocks `from..=to` in one `get_headers` call (the node's
/// own catch-up interface, at most `BATCH` per call). Reads the chain in a
/// few dozen requests instead of one per block, inside the node's RPC rate
/// limit. Node-local: the public gateway does not forward this method.
async fn timestamps(client: &HttpClient, from: u64, to: u64) -> Option<Vec<u64>> {
    let headers: Vec<String> = client
        .request("get_headers", rpc_params![from, to])
        .await
        .ok()?;
    headers
        .iter()
        .map(|h| {
            let bytes = h.get(TIMESTAMP_AT.start * 2..TIMESTAMP_AT.end * 2)?;
            let mut out = [0u8; 8];
            for (i, b) in out.iter_mut().enumerate() {
                *b = u8::from_str_radix(bytes.get(i * 2..i * 2 + 2)?, 16).ok()?;
            }
            Some(u64::from_le_bytes(out))
        })
        .collect()
}

async fn notify(http: &reqwest::Client, url: &str, text: String) {
    if let Err(e) = http.post(url).body(text).send().await {
        eprintln!("status: alert not delivered: {e}");
    }
}

/// Follows the chain into the shared timeline, alerting on state changes.
async fn follow(client: HttpClient, shared: Arc<Shared>, ntfy: Option<String>) {
    let http = reqwest::Client::new();
    let mut alerted = false;
    loop {
        let next = shared
            .timeline
            .lock()
            .map_or(None, |t| t.height())
            .map_or(0, |h| h + 1);
        if let Some(tip) = height(&client).await {
            shared.tip.store(tip, Ordering::Relaxed);
            // In header batches, applied strictly in height order.
            let mut h = next;
            while h <= tip {
                let end = (h + BATCH - 1).min(tip);
                let Some(stamps) = timestamps(&client, h, end).await else {
                    break;
                };
                if let Ok(mut t) = shared.timeline.lock() {
                    for (x, ts) in (h..=end).zip(stamps) {
                        t.observe(x, ts);
                    }
                }
                h = end + 1;
            }
        }
        let report = shared
            .caught_up()
            .and_then(|_| shared.timeline.lock().ok().and_then(|t| t.report(now())));
        if let (Some(r), Some(url)) = (report, ntfy.as_deref()) {
            if r.halted_now && !alerted {
                notify(
                    &http,
                    url,
                    format!(
                        "maya-testnet-1 halted: no block for {} s (last height {})",
                        r.since_last_block, r.height
                    ),
                )
                .await;
                alerted = true;
            } else if !r.halted_now && alerted {
                notify(
                    &http,
                    url,
                    format!("maya-testnet-1 producing again at height {}", r.height),
                )
                .await;
                alerted = false;
            }
        }
        tokio::time::sleep(POLL).await;
    }
}

async fn api(State(shared): State<Arc<Shared>>) -> impl IntoResponse {
    if shared.caught_up().is_none() {
        let seen = shared
            .timeline
            .lock()
            .map_or(None, |t| t.height())
            .unwrap_or(0);
        return Json(
            serde_json::json!({ "syncing": true, "height": seen, "tip": shared.tip.load(Ordering::Relaxed) }),
        );
    }
    let report = shared.timeline.lock().ok().and_then(|t| t.report(now()));
    Json(serde_json::to_value(report).unwrap_or(serde_json::Value::Null))
}

async fn page() -> Html<&'static str> {
    Html(include_str!("page.html"))
}

#[tokio::main]
async fn main() {
    let a = match args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let client = match HttpClientBuilder::default().build(&a.node) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("status: node client: {e}");
            std::process::exit(1);
        }
    };
    let shared = Arc::new(Shared {
        timeline: Mutex::new(Timeline::new(a.threshold)),
        tip: AtomicU64::new(u64::MAX),
    });
    tokio::spawn(follow(client, Arc::clone(&shared), a.ntfy));
    let app = Router::new()
        .route("/", get(page))
        .route("/api/status", get(api))
        .layer(tower_http::cors::CorsLayer::new().allow_origin(tower_http::cors::Any))
        .with_state(shared);
    let listener = match tokio::net::TcpListener::bind(a.listen).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("status: bind {}: {e}", a.listen);
            std::process::exit(1);
        }
    };
    println!("maya-status on http://{} following {}", a.listen, a.node);
    if let Err(e) = axum::serve(listener, app).await {
        eprintln!("status: {e}");
    }
}
