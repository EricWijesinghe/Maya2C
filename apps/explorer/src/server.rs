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

use crate::error::{ExplorerError, Result};
use crate::indexer::IndexEvent;
use crate::model::{HashratePoint, NetworkStats, average_block_time, estimate_hashrate};
use crate::store::BlockStore;
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
pub fn router(state: AppState) -> Router {
    let router = Router::new()
        .route("/", get(dashboard))
        .route("/blocks", get(blocks))
        .route("/blocks/{height}", get(block_detail))
        .route("/tx", get(transaction_search))
        .route("/tx/{txid}", get(transaction_detail))
        .route("/account", get(account))
        .route("/api/stats", get(api_stats))
        .route("/api/blocks", get(api_blocks))
        .route("/api/blocks/{height}", get(api_block))
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

    router.with_state(state)
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
    let points = gather_hashrate(&state.store).await?;
    let blocks = state.store.latest_blocks(LIST_LIMIT).await?;
    Ok(html(ui::dashboard_page(stats, points, blocks)))
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
    let address = query.address.unwrap_or_default();
    if address.is_empty() {
        return Ok(html(ui::account_page(
            String::new(),
            None,
            None,
            Vec::new(),
            None,
        )));
    }

    // Validate before spending a round trip on it.
    if hex::decode(&address).map(|b| b.len()) != Ok(32) {
        return Ok(html_status(
            StatusCode::BAD_REQUEST,
            ui::account_page(
                address,
                None,
                None,
                Vec::new(),
                Some("An address is 64 hexadecimal characters.".to_string()),
            ),
        ));
    }

    let transactions = state
        .store
        .transactions_by_sender(&address, LIST_LIMIT)
        .await?;

    // Balance comes from the node, live. A balance as of the last indexed
    // block would be presented as current and quietly be wrong.
    let (balance, nonce, error) = match &state.node_rpc {
        Some(url) => match fetch_account(url, &address).await {
            Ok((balance, nonce)) => (Some(balance), Some(nonce), None),
            Err(error) => (None, None, Some(format!("Node unavailable: {error}"))),
        },
        None => (
            None,
            None,
            Some("No node endpoint configured; balances unavailable.".to_string()),
        ),
    };

    Ok(html(ui::account_page(
        address,
        balance,
        nonce,
        transactions,
        error,
    )))
}

/// Reads a live balance from the node.
async fn fetch_account(url: &str, address: &str) -> Result<(u64, u64)> {
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

    let app = router(state);
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
