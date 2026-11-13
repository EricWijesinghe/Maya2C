//! Axum application: pages, JSON API, and WebSocket endpoints.
//!
//! ## Live updates
//!
//! WebSocket handlers subscribe to the indexer's broadcast channel. A slow
//! browser falls behind and its receiver reports `Lagged`; the handler skips
//! ahead rather than stalling. Indexing must never block on a client — a single
//! wedged tab would otherwise stop the explorer indexing the chain.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use tokio::sync::broadcast;
use tower_http::services::{ServeDir, ServeFile};

use maya_web_security::rate_limit::RateLimit;

use crate::api::node::{NoNode, NodeSource, RpcNode};
use crate::api::{self, ApiState, LiveCache};
use crate::error::{ExplorerError, Result};
use crate::indexer::IndexEvent;
use crate::model::{HashratePoint, NetworkStats, average_block_time, estimate_hashrate};
use crate::store::BlockStore;
use maya_address::Network;

use crate::ui;

/// Blocks shown in list views.
const LIST_LIMIT: i64 = 20;

/// Blocks sampled for the hashrate estimate.
///
/// Wide enough that one lucky block does not dominate, short enough to still
/// track a real change in hashpower.
const HASHRATE_WINDOW: i64 = 30;

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    /// Indexed data.
    pub store: Arc<dyn BlockStore>,
    /// Live indexing events.
    pub events: broadcast::Sender<IndexEvent>,
    /// Node RPC endpoint, for live account lookups.
    ///
    /// Balances are read straight from the node rather than indexed: an
    /// indexed balance is a balance as of the last indexed block, and showing
    /// a stale balance as current is worse than showing none.
    pub node_rpc: Option<String>,
    /// Directory holding the branding assets, if one was supplied.
    ///
    /// `None` mounts no `/assets` route at all, which is the honest state for a
    /// deployment that shipped no asset directory: the icons 404 either way, and
    /// a mounted route over a missing directory would only make the failure
    /// harder to find.
    pub assets: Option<PathBuf>,
    /// How `/api/v1` identifies a client for its rate limit (ADR-092).
    ///
    /// Behind a Cloudflare tunnel every request arrives from loopback, so the
    /// socket address would put all visitors in one bucket: deployments behind
    /// a proxy pass `CfConnectingIp` or `XForwardedForLast`.
    pub api_client_ip: maya_web_security::rate_limit::ClientIp,
}

/// Renders an HTML response.
fn html(body: String) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        body,
    )
        .into_response()
}

fn html_status(status: StatusCode, body: String) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        body,
    )
        .into_response()
}

impl IntoResponse for ExplorerError {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            // A store or upstream failure is the explorer's problem, not the
            // caller's, so it must not be reported as a client error.
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (
            status,
            Json(serde_json::json!({ "error": self.to_string() })),
        )
            .into_response()
    }
}

/// Builds the router.
///
/// # Panics
///
/// Never in practice: the API's rate limit is a constant that
/// [`api::default_rate_limit`] keeps valid.
#[allow(clippy::expect_used)]
pub fn router(state: AppState) -> Router {
    let api_state = ApiState {
        store: Arc::clone(&state.store),
        node: node_for(state.node_rpc.as_deref()),
        live: Arc::new(LiveCache::default()),
    };
    let limit = RateLimit {
        client_ip: state.api_client_ip,
        ..api::default_rate_limit()
    };
    let v1 = api::router(api_state, &limit).expect("default API rate limit is valid");

    let router = Router::new()
        .merge(v1)
        .route("/", get(dashboard))
        .route("/blocks", get(blocks))
        .route("/blocks/{height}", get(block_detail))
        .route("/tx", get(transaction_search))
        .route("/tx/{txid}", get(transaction_detail))
        .route("/account", get(account))
        .route("/account/{address}", get(account_path))
        .route("/mining", get(mining))
        .route("/validators", get(validators))
        .route("/api/validators", get(api_validators))
        .route("/api/stats", get(api_stats))
        .route("/api/blocks", get(api_blocks))
        .route("/api/blocks/{height}", get(api_block))
        .route(
            "/api/blocks/{height}/transactions",
            get(api_block_transactions),
        )
        .route("/api/tx/{txid}", get(api_transaction))
        .route("/api/hashrate", get(api_hashrate))
        .route("/ws/blocks", get(ws_blocks))
        .route("/ws/txs", get(ws_txs))
        .route("/health", get(health));

    // `/favicon.ico` is served at the root as well as under `/assets`, because
    // browsers request that exact path on their own regardless of what the
    // document's `<link>` tags say, and a 404 there shows up in every visitor's
    // console.
    let router = match &state.assets {
        Some(dir) => router
            .nest_service("/assets", ServeDir::new(dir))
            .route_service("/favicon.ico", ServeFile::new(dir.join("favicon.ico"))),
        None => router,
    };

    // Any origin may read by default: everything here is public chain data,
    // there are no credentials, and it is how maya2c.dev shows the network live.
    // GET only. `MAYA2C_CORS_ALLOWED_ORIGINS` locks it to listed origins.
    let cors = maya_web_security::cors_from_env([axum::http::Method::GET]);
    let router = router
        .merge(maya_web_security::security_txt())
        .layer(axum::middleware::from_fn(api::respond::legacy_api))
        .layer(cors);
    maya_web_security::secure(router, CONTENT_SECURITY_POLICY).with_state(state)
}

/// The explorer's pages run their own inline `<script>` and `<style>` blocks
/// (`ui.rs`), and the dashboard sets `style` attributes from script.
// shortcut: 'unsafe-inline' on script-src and style-src, because the inline blocks and attributes are not hashed; hash or move them to /assets to drop it.
const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self' 'unsafe-inline'; \
style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; font-src 'self'; \
object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

/// The node the v1 API queries. Without a usable URL every live route answers
/// `node_unavailable` and history keeps working.
fn node_for(url: Option<&str>) -> Arc<dyn NodeSource> {
    match url.map(RpcNode::connect) {
        Some(Ok(node)) => Arc::new(node),
        Some(Err(error)) => {
            eprintln!("api: node endpoint unusable, live routes disabled: {error}");
            Arc::new(NoNode)
        }
        None => Arc::new(NoNode),
    }
}

// ---------------------------------------------------------------------------
// data assembly
// ---------------------------------------------------------------------------

/// Gathers the dashboard's summary figures.
async fn gather_stats(store: &Arc<dyn BlockStore>) -> Result<NetworkStats> {
    let window = store.latest_blocks(HASHRATE_WINDOW).await?;
    let height = store.latest_height().await?.unwrap_or(0);

    Ok(NetworkStats {
        height,
        blocks_indexed: store.block_count().await?,
        transactions_indexed: store.transaction_count().await?,
        hashrate: estimate_hashrate(&window),
        average_block_time: average_block_time(&window),
        difficulty_target: window
            .first()
            .map(|block| block.difficulty_target.clone())
            .unwrap_or_default(),
    })
}

/// Builds a rolling hashrate series.
///
/// Each point is estimated over a small sub-window rather than a single block:
/// one block's work divided by one interval is dominated by luck, and the chart
/// would be noise.
async fn gather_hashrate(store: &Arc<dyn BlockStore>) -> Result<Vec<HashratePoint>> {
    const SUB_WINDOW: usize = 5;

    let mut blocks = store.latest_blocks(HASHRATE_WINDOW * 2).await?;
    blocks.reverse();

    if blocks.len() < SUB_WINDOW {
        return Ok(Vec::new());
    }

    Ok(blocks
        .windows(SUB_WINDOW)
        .map(|window| {
            let last = &window[window.len() - 1];
            HashratePoint {
                height: last.height,
                timestamp: last.timestamp,
                hashrate: estimate_hashrate(window),
            }
        })
        .collect())
}

// ---------------------------------------------------------------------------
// pages
// ---------------------------------------------------------------------------

async fn dashboard(State(state): State<AppState>) -> Result<Response> {
    let stats = gather_stats(&state.store).await?;
    let blocks = state.store.latest_blocks(LIST_LIMIT).await?;
    Ok(html(ui::dashboard_page(stats, blocks)))
}

async fn blocks(State(state): State<AppState>) -> Result<Response> {
    let blocks = state.store.latest_blocks(LIST_LIMIT * 2).await?;
    Ok(html(ui::blocks_page(blocks)))
}

async fn block_detail(State(state): State<AppState>, Path(height): Path<i64>) -> Result<Response> {
    match state.store.block_by_height(height).await? {
        Some(block) => {
            let transactions = state.store.transactions_in_block(height).await?;
            Ok(html(ui::block_page(block, transactions)))
        }
        None => Ok(html_status(
            StatusCode::NOT_FOUND,
            ui::not_found_page(format!("No block at height {height}")),
        )),
    }
}

/// Query for the transaction search form.
#[derive(Debug, Deserialize)]
struct TxQuery {
    txid: Option<String>,
}

async fn transaction_search(
    State(state): State<AppState>,
    Query(query): Query<TxQuery>,
) -> Result<Response> {
    let txid = query.txid.unwrap_or_default();
    if txid.is_empty() {
        return Ok(html(ui::transaction_page(None, String::new())));
    }
    let found = state.store.transaction(&txid).await?;
    Ok(html(ui::transaction_page(found, txid)))
}

async fn transaction_detail(
    State(state): State<AppState>,
    Path(txid): Path<String>,
) -> Result<Response> {
    let found = state.store.transaction(&txid).await?;
    let status = if found.is_some() {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    };
    Ok(html_status(status, ui::transaction_page(found, txid)))
}

/// Query for the account lookup form.
#[derive(Debug, Deserialize)]
struct AccountQuery {
    address: Option<String>,
}

async fn account(
    State(state): State<AppState>,
    Query(query): Query<AccountQuery>,
) -> Result<Response> {
    lookup_account(&state, query.address.unwrap_or_default()).await
}

/// The same page at a clean address path, `/account/<address>`.
async fn account_path(
    State(state): State<AppState>,
    Path(address): Path<String>,
) -> Result<Response> {
    lookup_account(&state, address).await
}

/// Shows an account from an address typed as hex or as Bech32m (ADR-081).
async fn lookup_account(state: &AppState, input: String) -> Result<Response> {
    if input.is_empty() {
        return Ok(html(ui::account_page(
            String::new(),
            None,
            None,
            None,
            Vec::new(),
            None,
        )));
    }

    // Validate before spending a round trip on it.
    let (id, typed_network) = match parse_address(&input) {
        Ok(parsed) => parsed,
        Err(message) => {
            return Ok(html_status(
                StatusCode::BAD_REQUEST,
                ui::account_page(input, None, None, None, Vec::new(), Some(message)),
            ));
        }
    };
    let hex_id = hex::encode(id);

    let transactions = state
        .store
        .transactions_by_sender(&hex_id, LIST_LIMIT)
        .await?;

    // Balance comes from the node, live. A balance as of the last indexed
    // block would be presented as current and quietly be wrong.
    let (balance, nonce, balance_error, network, network_error) = match &state.node_rpc {
        Some(url) => {
            let (network, network_error) = match typed_network {
                Some(network) => (Some(network), None),
                // A hex address names no network; the node's chain id does.
                None => match fetch_network(url).await {
                    Ok(network) => (network, None),
                    Err(error) => (None, Some(format!("Bech32m form unavailable: {error}"))),
                },
            };
            match fetch_account(url, &hex_id).await {
                Ok((balance, nonce)) => (Some(balance), Some(nonce), None, network, network_error),
                Err(error) => (
                    None,
                    None,
                    Some(format!("Node unavailable: {error}")),
                    network,
                    network_error,
                ),
            }
        }
        None => (
            None,
            None,
            Some("No node endpoint configured; balances unavailable.".to_string()),
            typed_network,
            None,
        ),
    };
    let error = [balance_error, network_error]
        .into_iter()
        .flatten()
        .reduce(|a, b| format!("{a} {b}"));
    let text = network.map(|network| maya_address::encode(network, &id));

    Ok(html(ui::account_page(
        hex_id,
        text,
        balance,
        nonce,
        transactions,
        error,
    )))
}

/// Reads an address as 32 bytes, from its 64 hex characters or its Bech32m
/// text. Bech32m also names the network, so its text form is known without
/// asking the node.
fn parse_address(input: &str) -> std::result::Result<([u8; 32], Option<Network>), String> {
    if let Ok(bytes) = hex::decode(input)
        && let Ok(id) = <[u8; 32]>::try_from(bytes.as_slice())
    {
        return Ok((id, None));
    }
    maya_address::decode_any(input)
        .map(|(network, id)| (id, Some(network)))
        .map_err(|_| "An address is 64 hexadecimal characters or a Bech32m address.".to_string())
}

/// The network the node serves, from its chain id. `None` for a chain id that
/// has no registered text form.
pub async fn fetch_network(url: &str) -> Result<Option<Network>> {
    use jsonrpsee::core::client::ClientT;
    use jsonrpsee::http_client::HttpClientBuilder;
    use jsonrpsee::rpc_params;

    let client = HttpClientBuilder::default()
        .build(url)
        .map_err(|e| ExplorerError::NodeRpc(e.to_string()))?;
    let info: custom_l1_node::rpc::ChainInfo = client
        .request("get_chain_info", rpc_params![])
        .await
        .map_err(|e| ExplorerError::NodeRpc(e.to_string()))?;
    Ok(info.chain_id.as_deref().and_then(Network::for_chain_id))
}

/// Estimated hashrate and block pacing, with the series behind the chart.
async fn mining(State(state): State<AppState>) -> Result<Response> {
    let stats = gather_stats(&state.store).await?;
    let series = gather_hashrate(&state.store).await?;
    Ok(html(ui::mining_page(stats, series)))
}

/// The validator set, live from the node: a stale committee would be shown as
/// current and quietly be wrong.
async fn validators(State(state): State<AppState>) -> Result<Response> {
    let (info, error) = match &state.node_rpc {
        Some(url) => match fetch_validators(url).await {
            Ok(Some(info)) => (Some(info), None),
            Ok(None) => (
                None,
                Some("This chain has no staking, so no validator set.".to_string()),
            ),
            Err(error) => (
                None,
                Some(format!(
                    "Node unavailable or too old to list validators: {error}"
                )),
            ),
        },
        None => (None, Some("No node endpoint configured.".to_string())),
    };
    Ok(html(ui::validators_page(info, error)))
}

async fn api_validators(
    State(state): State<AppState>,
) -> Result<Json<Option<custom_l1_node::rpc::validators::ValidatorsInfo>>> {
    let url = state
        .node_rpc
        .as_deref()
        .ok_or_else(|| ExplorerError::NodeRpc("no node endpoint configured".to_string()))?;
    Ok(Json(fetch_validators(url).await?))
}

async fn fetch_validators(
    url: &str,
) -> Result<Option<custom_l1_node::rpc::validators::ValidatorsInfo>> {
    use jsonrpsee::core::client::ClientT;
    use jsonrpsee::http_client::HttpClientBuilder;
    use jsonrpsee::rpc_params;

    let client = HttpClientBuilder::default()
        .build(url)
        .map_err(|e| ExplorerError::NodeRpc(e.to_string()))?;
    client
        .request("get_validators", rpc_params![])
        .await
        .map_err(|e| ExplorerError::NodeRpc(e.to_string()))
}

/// Reads a live balance from the node.
async fn fetch_account(url: &str, address: &str) -> Result<(primitive_types::U256, u64)> {
    use jsonrpsee::core::client::ClientT;
    use jsonrpsee::http_client::HttpClientBuilder;
    use jsonrpsee::rpc_params;

    let client = HttpClientBuilder::default()
        .build(url)
        .map_err(|e| ExplorerError::NodeRpc(e.to_string()))?;

    let info: custom_l1_node::rpc::AccountInfo = client
        .request("get_balance", rpc_params![address])
        .await
        .map_err(|e| ExplorerError::NodeRpc(e.to_string()))?;

    Ok((info.balance, info.nonce))
}

// ---------------------------------------------------------------------------
// JSON API
// ---------------------------------------------------------------------------

async fn api_stats(State(state): State<AppState>) -> Result<Json<NetworkStats>> {
    Ok(Json(gather_stats(&state.store).await?))
}

async fn api_blocks(
    State(state): State<AppState>,
) -> Result<Json<Vec<crate::model::IndexedBlock>>> {
    Ok(Json(state.store.latest_blocks(LIST_LIMIT).await?))
}

async fn api_block(
    State(state): State<AppState>,
    Path(height): Path<i64>,
) -> Result<Json<crate::model::IndexedBlock>> {
    state
        .store
        .block_by_height(height)
        .await?
        .map(Json)
        .ok_or_else(|| ExplorerError::NotFound(format!("block {height}")))
}

/// Every transaction in a block, with its kind, summary and care flag.
async fn api_block_transactions(
    State(state): State<AppState>,
    Path(height): Path<i64>,
) -> Result<Json<Vec<crate::model::IndexedTx>>> {
    if state.store.block_by_height(height).await?.is_none() {
        return Err(ExplorerError::NotFound(format!("block {height}")));
    }
    Ok(Json(state.store.transactions_in_block(height).await?))
}

async fn api_transaction(
    State(state): State<AppState>,
    Path(txid): Path<String>,
) -> Result<Json<crate::model::IndexedTx>> {
    state
        .store
        .transaction(&txid)
        .await?
        .map(Json)
        .ok_or_else(|| ExplorerError::NotFound(format!("transaction {txid}")))
}

async fn api_hashrate(State(state): State<AppState>) -> Result<Json<Vec<HashratePoint>>> {
    Ok(Json(gather_hashrate(&state.store).await?))
}

async fn health() -> &'static str {
    "ok"
}

// ---------------------------------------------------------------------------
// WebSocket
// ---------------------------------------------------------------------------

async fn ws_blocks(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| stream_events(socket, state.events.subscribe(), Feed::Blocks))
}

async fn ws_txs(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| stream_events(socket, state.events.subscribe(), Feed::Transactions))
}

/// Which events a socket wants.
#[derive(Clone, Copy)]
enum Feed {
    Blocks,
    Transactions,
}

/// Forwards matching events to a socket until it closes.
async fn stream_events(
    mut socket: WebSocket,
    mut events: broadcast::Receiver<IndexEvent>,
    feed: Feed,
) {
    loop {
        match events.recv().await {
            Ok(event) => {
                let payload = match (&event, feed) {
                    (IndexEvent::Block(block), Feed::Blocks) => serde_json::to_string(block).ok(),
                    (IndexEvent::Transaction(tx), Feed::Transactions) => {
                        serde_json::to_string(tx).ok()
                    }
                    // A rollback matters to both feeds: a client showing an
                    // abandoned block should be told, not left with it.
                    (
                        IndexEvent::Rollback {
                            from_height,
                            removed,
                        },
                        _,
                    ) => serde_json::to_string(&serde_json::json!({
                        "rollback": { "from_height": from_height, "removed": removed }
                    }))
                    .ok(),
                    _ => None,
                };

                if let Some(text) = payload
                    && socket.send(Message::Text(text.into())).await.is_err()
                {
                    // The client is gone.
                    return;
                }
            }
            // A slow client fell behind. Skipping is correct: the alternative
            // is applying backpressure to the indexer, which must never wait
            // on a browser.
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

// ---------------------------------------------------------------------------
// serving
// ---------------------------------------------------------------------------

/// A running explorer.
pub struct ExplorerServer {
    /// Address actually bound. Reflects the assigned port when `:0` is used.
    pub address: SocketAddr,
    /// Handle to the serving task.
    pub handle: tokio::task::JoinHandle<()>,
}

/// Binds and serves the explorer.
///
/// Pass port `0` to let the OS assign one and read it back from
/// [`ExplorerServer::address`] — which is what makes the tests hermetic.
///
/// # Errors
///
/// Returns [`ExplorerError::Server`] if the address cannot be bound.
pub async fn serve(address: SocketAddr, state: AppState) -> Result<ExplorerServer> {
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|e| ExplorerError::Server(format!("binding {address}: {e}")))?;

    let bound = listener
        .local_addr()
        .map_err(|e| ExplorerError::Server(format!("resolving local address: {e}")))?;

    // Connect info gives the /api/v1 limiter the peer address: without it every
    // request in socket mode, and every request missing the proxy header in
    // cf/xff mode, has no client key and fails.
    let app = router(state).into_make_service_with_connect_info::<SocketAddr>();
    let handle = tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, app).await {
            eprintln!("explorer server stopped: {error}");
        }
    });

    Ok(ExplorerServer {
        address: bound,
        handle,
    })
}
