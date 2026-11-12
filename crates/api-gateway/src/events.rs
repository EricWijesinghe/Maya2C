//! Event subscriptions for app developers: new blocks and transactions,
//! filtered by address or asset, over a WebSocket or a webhook.
//!
//! # Where events come from
//!
//! The gateway is a JSON-RPC client of the node and keeps no index, so a
//! poller ([`spawn_poller`]) reads the tip with `get_chain_info` and each new
//! block with `get_block_by_height`, and publishes onto one
//! `tokio::sync::broadcast` channel. That is the shape the explorer's
//! `/ws/blocks` uses (`apps/explorer/src/server.rs`), including its rule for a
//! slow consumer: it skips what it missed rather than holding the feed back.
//! The explorer's feed is fed by its database indexer, which the gateway does
//! not have, so the pattern is shared rather than the code.
//!
//! # What it does not do
//!
//! History is not replayed: the feed starts at the tip the gateway first sees.
//! Subscriptions live in memory and are lost on restart. There is no rollback
//! event: the poller reports each height once, as the node reports it.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, FromRequestParts, Path, Query, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::error::GatewayError;
use crate::limit::{self, ClientIp};
use crate::node::NodeClient;
use crate::ssrf;
use crate::webhooks::{Retry, Status, Transport, Webhook, WebhookSecret};

/// Largest event sent. A transaction over it is sent as its id with
/// `"truncated": true`; the caller fetches the block for the rest.
pub const MAX_EVENT_BYTES: usize = 64 * 1024;
/// Largest webhook registration body accepted.
pub const MAX_REGISTRATION_BYTES: usize = 4 * 1024;
/// Largest frame a stream client may send; it has nothing to say.
const MAX_CLIENT_FRAME: usize = 1024;
/// Blocks fetched per poll, so a gateway far behind catches up in steps
/// rather than in one burst of node calls.
pub const MAX_BLOCKS_PER_POLL: u64 = 32;
/// How often a stream is pinged; three silent intervals close it.
const STREAM_PING: Duration = Duration::from_secs(20);
/// Longest a single send to a stream client may take.
const STREAM_SEND_TIMEOUT: Duration = Duration::from_secs(10);
/// Events buffered for consumers before the slowest starts skipping.
const FEED_CAPACITY: usize = 1024;
/// Longest asset name accepted in a filter.
const MAX_ASSET_LEN: usize = 64;

/// One chain event.
#[derive(Clone, Debug, Serialize)]
pub struct Event {
    /// `block:<height>` or `tx:<txid>`: what a receiver deduplicates on.
    pub id: String,
    /// `block` or `tx`.
    pub kind: &'static str,
    /// The block height.
    pub height: u64,
    /// The block header and transaction count, or the transaction.
    pub data: serde_json::Value,
    #[serde(skip)]
    addresses: Vec<String>,
    #[serde(skip)]
    assets: Vec<String>,
}

fn normalize(address: &str) -> String {
    address.strip_prefix("0x").unwrap_or(address).to_ascii_lowercase()
}

/// Addresses and assets a transaction touches. An output without an `asset`
/// field is the native coin: the node's JSON names no asset today.
fn touched(tx: &serde_json::Value) -> (Vec<String>, Vec<String>) {
    let outputs = tx["outputs"].as_array().map(Vec::as_slice).unwrap_or_default();
    let mut addresses: Vec<String> = tx["sender"].as_str().map(normalize).into_iter().collect();
    let mut assets: Vec<String> = tx["asset"].as_str().map(str::to_string).into_iter().collect();
    for output in outputs {
        addresses.extend(output["recipient"].as_str().map(normalize));
        assets.push(output["asset"].as_str().unwrap_or("native").to_string());
    }
    (addresses, assets)
}

impl Event {
    /// The events one block yields: the block, then each transaction.
    #[must_use]
    pub fn from_block(height: u64, block: &serde_json::Value) -> Vec<Self> {
        let txs = block["transactions"].as_array().map(Vec::as_slice).unwrap_or_default();
        let mut block_event = Self {
            id: format!("block:{height}"),
            kind: "block",
            height,
            data: serde_json::json!({
                "height": height, "header": block["header"], "tx_count": txs.len(),
            }),
            addresses: Vec::new(),
            assets: Vec::new(),
        };
        let mut events = Vec::with_capacity(txs.len() + 1);
        for tx in txs {
            let (addresses, assets) = touched(tx);
            block_event.addresses.extend(addresses.iter().cloned());
            block_event.assets.extend(assets.iter().cloned());
            let txid = tx["txid"].as_str().unwrap_or_default();
            let size = serde_json::to_vec(tx).map_or(usize::MAX, |b| b.len());
            let data = if size > MAX_EVENT_BYTES {
                serde_json::json!({ "txid": txid, "truncated": true })
            } else {
                tx.clone()
            };
            events.push(Self {
                id: format!("tx:{txid}"),
                kind: "tx",
                height,
                data,
                addresses,
                assets,
            });
        }
        events.insert(0, block_event);
        events
    }
}

/// Which events a subscriber wants. Empty matches everything; a block matches
/// if any of its transactions does.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Filter {
    /// Hex address, as sender or recipient.
    pub address: Option<String>,
    /// Asset name; `native` for the chain's coin.
    pub asset: Option<String>,
}

impl Filter {
    /// Checks and normalizes the filter.
    ///
    /// # Errors
    ///
    /// [`GatewayError::BadRequest`] for a malformed address or asset.
    pub fn validated(self) -> Result<Self, GatewayError> {
        let address = match self.address {
            Some(a) => Some(normalize(crate::rest::parse_address(&a)?)),
            None => None,
        };
        if let Some(asset) = &self.asset {
            let ok = !asset.is_empty()
                && asset.len() <= MAX_ASSET_LEN
                && asset.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
            if !ok {
                return Err(GatewayError::BadRequest("malformed asset".to_string()));
            }
        }
        Ok(Self { address, asset: self.asset })
    }

    /// Whether `event` passes.
    #[must_use]
    pub fn matches(&self, event: &Event) -> bool {
        self.address.as_ref().is_none_or(|a| event.addresses.contains(a))
            && self.asset.as_ref().is_none_or(|a| event.assets.contains(a))
    }
}

/// Limits and policy for subscriptions.
#[derive(Clone, Debug)]
pub struct EventsConfig {
    /// Plain-`http` webhook targets (development only).
    pub allow_http: bool,
    /// Webhooks one caller may hold, dead-lettered ones included.
    pub max_webhooks_per_caller: usize,
    /// Webhooks the gateway keeps in total.
    pub max_webhooks: usize,
    /// Open streams per caller.
    pub max_streams_per_caller: usize,
    /// Open streams in total.
    pub max_streams: usize,
    /// Delivery attempts per event before dead-lettering.
    pub max_attempts: u32,
    /// Wait after the first failed attempt; doubles each time.
    pub backoff_base: Duration,
    /// Longest wait between attempts.
    pub backoff_max: Duration,
    /// How callers are identified; must match the rate limiter's.
    pub client_ip: ClientIp,
}

impl Default for EventsConfig {
    fn default() -> Self {
        Self {
            allow_http: false,
            max_webhooks_per_caller: 5,
            max_webhooks: 1_000,
            max_streams_per_caller: 5,
            max_streams: 1_000,
            max_attempts: 5,
            backoff_base: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
            client_ip: ClientIp::Socket,
        }
    }
}

type WebhookTable = HashMap<String, (Arc<Webhook>, tokio::task::AbortHandle)>;

/// The feed and its subscriptions.
pub struct Events {
    config: EventsConfig,
    feed: broadcast::Sender<Arc<Event>>,
    transport: Arc<dyn Transport>,
    webhooks: Mutex<WebhookTable>,
    streams: Mutex<HashMap<IpAddr, usize>>,
}

fn locked<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Events {
    /// A hub delivering webhooks through `transport`.
    #[must_use]
    pub fn new(config: EventsConfig, transport: Arc<dyn Transport>) -> Arc<Self> {
        Arc::new(Self {
            config,
            feed: broadcast::channel(FEED_CAPACITY).0,
            transport,
            webhooks: Mutex::new(HashMap::new()),
            streams: Mutex::new(HashMap::new()),
        })
    }

    /// Sends `event` to every subscriber. No subscriber is not an error.
    pub fn publish(&self, event: Event) {
        let _ = self.feed.send(Arc::new(event));
    }

    /// A receiver for the raw feed.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Event>> {
        self.feed.subscribe()
    }

    /// Open streams.
    #[must_use]
    pub fn stream_count(&self) -> usize {
        locked(&self.streams).values().sum()
    }

    fn open_stream(self: &Arc<Self>, caller: IpAddr) -> Result<StreamSlot, GatewayError> {
        let mut streams = locked(&self.streams);
        let total: usize = streams.values().sum();
        let mine = streams.get(&caller).copied().unwrap_or_default();
        if mine >= self.config.max_streams_per_caller || total >= self.config.max_streams {
            return Err(GatewayError::TooManySubscriptions("streams".to_string()));
        }
        streams.insert(caller, mine + 1);
        Ok(StreamSlot { events: Arc::clone(self), caller })
    }

    fn check_room(&self, webhooks: &WebhookTable, caller: IpAddr) -> Result<(), GatewayError> {
        let mine = webhooks.values().filter(|(h, _)| h.owner == caller).count();
        if mine >= self.config.max_webhooks_per_caller || webhooks.len() >= self.config.max_webhooks {
            return Err(GatewayError::TooManySubscriptions("webhooks".to_string()));
        }
        Ok(())
    }

    fn retry(&self) -> Retry {
        Retry {
            max_attempts: self.config.max_attempts.max(1),
            base: self.config.backoff_base,
            max: self.config.backoff_max,
        }
    }
}

/// A held stream slot, released when the stream ends.
struct StreamSlot {
    events: Arc<Events>,
    caller: IpAddr,
}

impl Drop for StreamSlot {
    fn drop(&mut self) {
        let mut streams = locked(&self.events.streams);
        if let Some(count) = streams.get_mut(&self.caller) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                streams.remove(&self.caller);
            }
        }
    }
}

/// One poll: publishes every block after the last one seen, up to
/// [`MAX_BLOCKS_PER_POLL`]. The first call only records the tip. Returns the
/// number of events published.
///
/// # Errors
///
/// The node's error; `next` then still names the first unpublished block.
pub async fn poll_once(
    node: &dyn NodeClient,
    events: &Events,
    next: &mut Option<u64>,
) -> Result<usize, GatewayError> {
    let tip = node.get_chain_info().await?.height;
    let Some(start) = *next else {
        *next = Some(tip.saturating_add(1));
        return Ok(0);
    };
    let end = tip.min(start.saturating_add(MAX_BLOCKS_PER_POLL - 1));
    let mut published = 0;
    for height in start..=end {
        let block = node.get_block_by_height(height).await?;
        for event in Event::from_block(height, &block) {
            events.publish(event);
            published += 1;
        }
        *next = Some(height.saturating_add(1));
    }
    Ok(published)
}

/// Polls the node every `interval` for as long as the gateway runs.
pub fn spawn_poller(
    node: Arc<dyn NodeClient>,
    events: Arc<Events>,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(interval);
        let mut next = None;
        loop {
            tick.tick().await;
            if let Err(error) = poll_once(node.as_ref(), &events, &mut next).await {
                tracing::warn!(%error, "event poll failed; retrying next tick");
            }
        }
    })
}

/// The subscription routes.
pub fn router(events: Arc<Events>) -> Router {
    Router::new()
        .route("/v1/subscribe", get(stream))
        .route("/v1/webhooks", axum::routing::post(register))
        .route("/v1/webhooks/{id}", get(show).delete(remove))
        .layer(DefaultBodyLimit::max(MAX_REGISTRATION_BYTES))
        .with_state(events)
}

/// The caller, keyed the way the rate limiter keys it, except that an IPv6
/// caller counts as its /64: one host routinely holds the whole prefix.
struct Caller(IpAddr);

fn cap_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => {
            let mut segments = v6.segments();
            segments[4..].fill(0);
            IpAddr::V6(segments.into())
        }
        v4 @ IpAddr::V4(_) => v4,
    }
}

impl FromRequestParts<Arc<Events>> for Caller {
    type Rejection = GatewayError;

    async fn from_request_parts(parts: &mut Parts, events: &Arc<Events>) -> Result<Self, GatewayError> {
        limit::client_address(events.config.client_ip, &parts.headers, &parts.extensions)
            .map(|ip| Caller(cap_key(ip)))
            .ok_or_else(|| GatewayError::BadRequest("caller address unknown".to_string()))
    }
}

async fn stream(
    State(events): State<Arc<Events>>,
    Caller(caller): Caller,
    Query(filter): Query<Filter>,
    ws: WebSocketUpgrade,
) -> Result<Response, GatewayError> {
    let filter = filter.validated()?;
    let slot = events.open_stream(caller)?;
    let feed = events.subscribe();
    Ok(ws
        .max_message_size(MAX_CLIENT_FRAME)
        .max_frame_size(MAX_CLIENT_FRAME)
        .on_upgrade(move |socket| forward(socket, feed, filter, slot)))
}

/// Forwards matching events until the client leaves; the slot goes with it.
async fn forward(
    socket: WebSocket,
    mut feed: broadcast::Receiver<Arc<Event>>,
    filter: Filter,
    _slot: StreamSlot,
) {
    let (mut sink, mut incoming) = socket.split();
    let mut ping = tokio::time::interval(STREAM_PING);
    let mut last_heard = tokio::time::Instant::now();
    loop {
        tokio::select! {
            received = feed.recv() => match received {
                Ok(event) if filter.matches(&event) => {
                    let Ok(text) = serde_json::to_string(&*event) else { continue };
                    if !send(&mut sink, Message::Text(text.into())).await {
                        return;
                    }
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return,
            },
            // Anything from the client is only a sign of life; its end is
            // what matters.
            message = incoming.next() => match message {
                None | Some(Err(_) | Ok(Message::Close(_))) => return,
                Some(Ok(_)) => last_heard = tokio::time::Instant::now(),
            },
            // A client that stops reading or vanishes half-open gives its
            // slot back: pings go unanswered, or a send stalls.
            _ = ping.tick() => {
                let silent = last_heard.elapsed() > STREAM_PING * 3;
                if silent || !send(&mut sink, Message::Ping(Vec::new().into())).await {
                    return;
                }
            }
        }
    }
}

async fn send(sink: &mut futures_util::stream::SplitSink<WebSocket, Message>, message: Message) -> bool {
    matches!(tokio::time::timeout(STREAM_SEND_TIMEOUT, sink.send(message)).await, Ok(Ok(())))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Registration {
    url: String,
    address: Option<String>,
    asset: Option<String>,
}

async fn register(
    State(events): State<Arc<Events>>,
    Caller(caller): Caller,
    Json(request): Json<Registration>,
) -> Result<(StatusCode, Json<serde_json::Value>), GatewayError> {
    let url = ssrf::check_url(&request.url, events.config.allow_http)?;
    // Before the DNS lookup, so a caller over its cap cannot make the gateway
    // resolve names; checked again under the lock when inserting.
    events.check_room(&locked(&events.webhooks), caller)?;
    let filter = Filter { address: request.address, asset: request.asset }.validated()?;
    // Refused now if the name already resolves somewhere private; resolved
    // again before every delivery.
    events.transport.admit(&url).await?;
    let secret = WebhookSecret::generate()?;
    let mut id = [0u8; 16];
    getrandom::fill(&mut id).map_err(|_| GatewayError::Upstream("entropy unavailable".to_string()))?;
    let id = hex::encode(id);
    let secret_hex = secret.to_hex();
    let hook = Arc::new(Webhook {
        id: id.clone(),
        url,
        filter,
        owner: caller,
        secret,
        status: Mutex::new(Status { state: "active", ..Status::default() }),
    });
    let mut webhooks = locked(&events.webhooks);
    events.check_room(&webhooks, caller)?;
    // Subscribed before the response, so no event after it is missed.
    let task = tokio::spawn(deliver_all(Arc::clone(&events), Arc::clone(&hook), events.subscribe()));
    webhooks.insert(id.clone(), (hook, task.abort_handle()));
    drop(webhooks);
    tracing::info!(webhook = %id, "webhook registered");
    let body = serde_json::json!({ "id": id, "secret": secret_hex.as_str() });
    Ok((StatusCode::CREATED, Json(body)))
}

async fn deliver_all(events: Arc<Events>, hook: Arc<Webhook>, mut feed: broadcast::Receiver<Arc<Event>>) {
    let retry = events.retry();
    loop {
        match feed.recv().await {
            Ok(event) if hook.filter.matches(&event) => {
                let body = serde_json::json!({ "subscription": hook.id, "event": &*event });
                let Ok(body) = serde_json::to_vec(&body) else { continue };
                if !hook.deliver(events.transport.as_ref(), &event.id, body, &retry).await {
                    return;
                }
            }
            Ok(_) => {}
            Err(broadcast::error::RecvError::Lagged(n)) => {
                hook.status.lock().unwrap_or_else(PoisonError::into_inner).skipped += n;
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// The webhook `id`, if `headers` carry its secret. A wrong secret and a
/// missing webhook are the same 404, so neither confirms an id exists.
fn authorized(events: &Events, id: &str, headers: &HeaderMap) -> Result<Arc<Webhook>, GatewayError> {
    let presented = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(GatewayError::NotFound)?;
    locked(&events.webhooks)
        .get(id)
        .filter(|(hook, _)| hook.secret.matches(presented))
        .map(|(hook, _)| Arc::clone(hook))
        .ok_or(GatewayError::NotFound)
}

async fn show(
    State(events): State<Arc<Events>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, GatewayError> {
    let hook = authorized(&events, &id, &headers)?;
    let mut body = serde_json::to_value(hook.status())
        .map_err(|_| GatewayError::Upstream("status encoding".to_string()))?;
    body["id"] = hook.id.clone().into();
    body["url"] = hook.url.as_str().into();
    body["filter"] = serde_json::to_value(&hook.filter).unwrap_or_default();
    Ok(Json(body))
}

async fn remove(
    State(events): State<Arc<Events>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, GatewayError> {
    authorized(&events, &id, &headers)?;
    if let Some((_, task)) = locked(&events.webhooks).remove(&id) {
        task.abort();
    }
    Ok(StatusCode::NO_CONTENT)
}
