//! `maya-status`: watches a node, derives uptime from block timestamps, and
//! serves a status page and `/api/status`.
//!
//! ```text
//! maya-status --node http://127.0.0.1:8545 --listen 127.0.0.1:3100 \
//!             [--threshold 60] [--ntfy https://ntfy.sh/<private-topic>]
//! ```
//!
//! `--watch NAME=URL` (repeatable) adds a node to the live network view at
//! `/live` (`/api/live`), polled every second; without it the view watches
//! `--node` alone, as `seed`.
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
use maya_status::live::{BftStatus, Live, NetworkView, NodePoll};

const POLL: Duration = Duration::from_secs(2);

/// How often the live view polls each watched node: "within seconds" of a
/// peer connecting, it is on the page.
const LIVE_POLL: Duration = Duration::from_secs(1);

/// A watched node slower than this counts as not answering this poll.
const LIVE_TIMEOUT: Duration = Duration::from_millis(800);

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
    watch: Vec<(String, String)>,
}

fn args() -> Result<Args, String> {
    let mut a = Args {
        node: "http://127.0.0.1:8545".into(),
        listen: "127.0.0.1:3100".parse().map_err(|e| format!("{e}"))?,
        threshold: 60,
        ntfy: None,
        watch: Vec::new(),
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
            "--watch" => {
                let v = value()?;
                let (name, url) = v.split_once('=').ok_or("--watch takes NAME=URL")?;
                a.watch.push((name.to_owned(), url.to_owned()));
            }
            "--help" | "-h" => {
                return Err(
                    "usage: maya-status --node URL --listen ADDR [--threshold SECS] [--ntfy URL] [--watch NAME=URL]..."
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

async fn live_page() -> Html<&'static str> {
    Html(include_str!("live.html"))
}

async fn live_api(State(live): State<Arc<Mutex<Live>>>) -> impl IntoResponse {
    let view = live.lock().map(|l| l.view().clone()).unwrap_or_default();
    Json(serde_json::to_value(view).unwrap_or(serde_json::Value::Null))
}

/// One watched node's three answers; any it did not give stays `None`.
async fn poll_node(name: &str, client: &HttpClient) -> NodePoll {
    let (bft, view, height) = tokio::join!(
        client.request::<BftStatus, _>("get_bft_status", rpc_params![]),
        client.request::<NetworkView, _>("get_peers", rpc_params![]),
        self::height(client),
    );
    NodePoll {
        name: name.to_owned(),
        bft: bft.ok(),
        view: view.ok(),
        height,
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Polls every watched node each [`LIVE_POLL`] into the live view, all of
/// them at once, so one slow node never delays the others.
async fn watch(nodes: Vec<(String, HttpClient)>, live: Arc<Mutex<Live>>) {
    let mut tick = tokio::time::interval(LIVE_POLL);
    loop {
        tick.tick().await;
        let mut set = tokio::task::JoinSet::new();
        for (i, (name, client)) in nodes.iter().enumerate() {
            let (name, client) = (name.clone(), client.clone());
            set.spawn(async move { (i, poll_node(&name, &client).await) });
        }
        // A poll task that panicked leaves its node reading as silent.
        let mut polls: Vec<NodePoll> = nodes
            .iter()
            .map(|(name, _)| NodePoll {
                name: name.clone(),
                ..NodePoll::default()
            })
            .collect();
        while let Some(joined) = set.join_next().await {
            if let Ok((i, p)) = joined {
                polls[i] = p;
            }
        }
        if let Ok(mut l) = live.lock() {
            l.observe(now_ms(), &polls);
        }
    }
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

    let watch_list = if a.watch.is_empty() {
        vec![("seed".to_owned(), a.node.clone())]
    } else {
        a.watch.clone()
    };
    let mut watched = Vec::new();
    for (name, url) in watch_list {
        match HttpClientBuilder::default()
            .request_timeout(LIVE_TIMEOUT)
            .build(&url)
        {
            Ok(c) => watched.push((name, c)),
            Err(e) => eprintln!("status: --watch {name}: {e}"),
        }
    }
    let live = Arc::new(Mutex::new(Live::new("maya-testnet-1")));
    tokio::spawn(watch(watched, Arc::clone(&live)));

    let live_routes = Router::new()
        .route("/live", get(live_page))
        .route("/api/live", get(live_api))
        .with_state(live);
    let app = Router::new()
        .route("/", get(page))
        .route("/api/status", get(api))
        .with_state(shared)
        .merge(live_routes)
        .layer(tower_http::cors::CorsLayer::new().allow_origin(tower_http::cors::Any));
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
