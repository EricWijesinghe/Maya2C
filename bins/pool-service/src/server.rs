//! The dashboard and JSON API server.
//!
//! An Axum application over [`crate::state::PoolState`], rendering
//! [`crate::ui`] and serving [`crate::api`]. Structurally the explorer's
//! server, and for the same reasons — including the one about slow clients:
//! a WebSocket subscriber that falls behind reports `Lagged` and is skipped
//! ahead rather than waited for. Share accounting must never block on a
//! browser.
//!
//! ## This port is not the mining port
//!
//! Miners connect over the post-quantum Stratum transport on its own listener.
//! Nothing here can open a channel, move a target, or start a payout — every
//! route is a read. A dashboard that could act on the pool would be a second
//! and much weaker door into the parts that handle money.
//!
//! ## And it is not the metrics port either
//!
//! Three listeners, three audiences: miners, operators, and Prometheus. The
//! exporter carries per-pool operational data and stays inside the pod network
//! (`src/metrics/mod.rs` gives that reasoning); this one is the page a miner
//! opens.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use tokio::sync::broadcast;

use crate::api::{self, BlockList, MinerView, PayoutList, PoolStatus};
use crate::error::PoolError;
use crate::model::now_millis;
use crate::state::{PoolEvent, PoolState};
use crate::ui;

/// Rows shown in list views.
const LIST_LIMIT: usize = 50;

/// A running dashboard.
#[derive(Debug)]
pub struct DashboardServer {
    /// Address actually bound, which matters when the caller asked for port 0.
    pub address: SocketAddr,
}

impl IntoResponse for PoolError {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            // A ledger, chain, or treasury failure is the pool's problem, not
            // the caller's, and must not be reported as a client error.
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (
            status,
            Json(serde_json::json!({ "error": self.to_string() })),
        )
            .into_response()
    }
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

/// Builds the router.
pub fn router(state: Arc<PoolState>) -> Router {
    Router::new()
        .route("/", get(overview))
        .route("/blocks", get(blocks))
        .route("/payouts", get(payouts))
        .route("/miner/{address}", get(miner))
        .route("/api/status", get(status_json))
        .route("/api/miner/{address}", get(miner_json))
        .route("/api/blocks", get(blocks_json))
        .route("/api/payouts", get(payouts_json))
        .route("/healthz", get(|| async { "ok" }))
        .route("/ws", get(websocket))
        .with_state(state)
}

/// Binds the dashboard and serves it on a background task.
///
/// # Errors
///
/// Returns [`PoolError::Server`] if the address cannot be bound.
pub async fn serve(
    address: SocketAddr,
    state: Arc<PoolState>,
) -> crate::error::Result<DashboardServer> {
    let listener = tokio::net::TcpListener::bind(address).await?;
    let bound = listener.local_addr()?;
    let app = router(state);

    tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, app).await {
            eprintln!("pool: the dashboard stopped: {error}");
        }
    });

    Ok(DashboardServer { address: bound })
}

/// Gathers pool-wide status.
fn build_status(state: &PoolState) -> crate::error::Result<PoolStatus> {
    let now = now_millis();
    let channels = state.channels()?.len();
    let (workers, measured_hashrate, reported_power_milliwatts) = {
        let telemetry = state.telemetry()?;
        (
            telemetry.len(),
            telemetry.total_hashrate(now),
            telemetry.total_reported_power_milliwatts(),
        )
    };

    let (current_job, height) = {
        let jobs = state.jobs()?;
        match jobs.current() {
            Some(job) => (Some(job.id), Some(job.height)),
            None => (None, None),
        }
    };

    let mut unpaid = 0u64;
    let mut immature = 0u64;
    for (_, balance) in state.ledger.balances()? {
        unpaid = unpaid.saturating_add(balance.unpaid);
        immature = immature.saturating_add(balance.immature);
    }

    Ok(PoolStatus {
        measured_hashrate,
        channels,
        workers,
        reported_power_milliwatts,
        unpaid,
        immature,
        open_batches: state.ledger.open_batches()?.len(),
        current_job,
        height,
        pplns_factor: state.config.pplns_factor,
        fee_rate: state.config.fee_rate,
        min_payout: state.config.min_payout,
        reward_per_block: state.config.miner_share(state.config.reward_per_block),
    })
}

/// The overview page.
async fn overview(State(state): State<Arc<PoolState>>) -> crate::error::Result<Response> {
    let now = now_millis();
    let status = build_status(&state)?;
    let workers = state.telemetry()?.snapshot(now);
    let blocks = state.ledger.recent_blocks(LIST_LIMIT)?;

    Ok(html(ui::overview_page(
        status.measured_hashrate,
        status.channels,
        workers,
        status.reported_power_milliwatts,
        blocks,
    )))
}

/// The found-blocks page.
async fn blocks(State(state): State<Arc<PoolState>>) -> crate::error::Result<Response> {
    Ok(html(ui::blocks_page(
        state.ledger.recent_blocks(LIST_LIMIT)?,
    )))
}

/// The payouts page.
async fn payouts(State(state): State<Arc<PoolState>>) -> crate::error::Result<Response> {
    Ok(html(ui::payouts_page(
        state.ledger.recent_batches(LIST_LIMIT)?,
    )))
}

/// One miner's page.
async fn miner(
    State(state): State<Arc<PoolState>>,
    Path(address): Path<String>,
) -> crate::error::Result<Response> {
    let parsed = api::parse_address(&address)?;
    let balance = state.ledger.balance(&parsed)?;
    let workers = state.telemetry()?.workers_of(&parsed, now_millis());

    // A miner with no rigs and no balance is a mistyped address, and saying so
    // is more useful than an empty page that looks like a broken pool.
    if workers.is_empty() && balance == crate::ledger::MinerBalance::default() {
        return Ok((
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            ui::error_page(
                "Unknown miner",
                "No shares or balance have been recorded for this address.",
            ),
        )
            .into_response());
    }

    Ok(html(ui::miner_page(hex::encode(parsed), balance, workers)))
}

/// Pool-wide status, as JSON.
async fn status_json(
    State(state): State<Arc<PoolState>>,
) -> crate::error::Result<Json<PoolStatus>> {
    Ok(Json(build_status(&state)?))
}

/// One miner's account, as JSON.
async fn miner_json(
    State(state): State<Arc<PoolState>>,
    Path(address): Path<String>,
) -> crate::error::Result<Json<MinerView>> {
    let parsed = api::parse_address(&address)?;
    let balance = state.ledger.balance(&parsed)?;
    let workers = state.telemetry()?.workers_of(&parsed, now_millis());
    Ok(Json(api::miner_view(parsed, balance, workers)))
}

/// Found blocks, as JSON.
async fn blocks_json(State(state): State<Arc<PoolState>>) -> crate::error::Result<Json<BlockList>> {
    Ok(Json(BlockList {
        blocks: state.ledger.recent_blocks(LIST_LIMIT)?,
    }))
}

/// Payout batches, as JSON.
async fn payouts_json(
    State(state): State<Arc<PoolState>>,
) -> crate::error::Result<Json<PayoutList>> {
    Ok(Json(PayoutList {
        batches: state.ledger.recent_batches(LIST_LIMIT)?,
    }))
}

/// Upgrades to a WebSocket carrying pool events.
async fn websocket(
    State(state): State<Arc<PoolState>>,
    upgrade: WebSocketUpgrade,
) -> impl IntoResponse {
    let events = state.events.subscribe();
    upgrade.on_upgrade(move |socket| pump(socket, events))
}

/// Forwards events to one browser until it goes away.
async fn pump(mut socket: WebSocket, mut events: broadcast::Receiver<PoolEvent>) {
    loop {
        let event = match events.recv().await {
            Ok(event) => event,
            // A slow tab falls behind. Skipping ahead is the only acceptable
            // response: waiting for it would put a browser between a share and
            // its credit.
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        };

        let Ok(payload) = serde_json::to_string(&event) else {
            continue;
        };
        if socket.send(WsMessage::Text(payload.into())).await.is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    use custom_l1_node::crypto::dag::registry::{CacheRegistry, DagConfig};

    use crate::config::PoolConfig;
    use crate::ledger::memory::MemoryLedger;
    use crate::ledger::{BlockState, FoundBlock};
    use crate::metrics::PoolMetrics;
    use crate::model::PayoutEntry;

    const ALICE: [u8; 32] = [0xAA; 32];

    fn state() -> Arc<PoolState> {
        Arc::new(PoolState::new(
            PoolConfig {
                reward_per_block: 1_000,
                fee_rate: 0.02,
                ..PoolConfig::default()
            },
            Arc::new(MemoryLedger::new()),
            Arc::new(PoolMetrics::new()),
            Arc::new(CacheRegistry::new(DagConfig::NEVER)),
        ))
    }

    /// Issues a request against the router and returns status and body.
    async fn get(state: Arc<PoolState>, path: &str) -> (StatusCode, String) {
        use tower::ServiceExt;

        let response = router(state)
            .oneshot(
                axum::http::Request::builder()
                    .uri(path)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&bytes).to_string())
    }

    #[tokio::test]
    async fn the_overview_renders_on_an_empty_pool() {
        // The state a pool is in for its first minutes, and the one a broken
        // dashboard most often fails on.
        let (status, body) = get(state(), "/").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Measured hashrate"));
    }

    #[tokio::test]
    async fn status_reports_the_reward_the_operator_configured() {
        // On this chain the reward is policy, not a consensus constant, so a
        // miner cannot infer it and the API has to state it.
        let (status, body) = get(state(), "/api/status").await;
        assert_eq!(status, StatusCode::OK);

        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["reward_per_block"], 980, "1000 less a 2% fee");
        assert_eq!(json["fee_rate"], 0.02);
    }

    #[tokio::test]
    async fn a_malformed_address_is_a_client_error_with_a_reason() {
        let (status, body) = get(state(), "/api/miner/not-hex").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("not hex"));
    }

    #[tokio::test]
    async fn an_unknown_miner_is_a_not_found_rather_than_an_empty_page() {
        let (status, body) = get(state(), &format!("/miner/{}", hex::encode(ALICE))).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("Unknown miner"));
    }

    #[tokio::test]
    async fn a_miner_with_a_balance_is_rendered() {
        let state = state();
        state
            .ledger
            .record_block(
                &FoundBlock {
                    id: "block-a".to_string(),
                    height: 10,
                    sequence: 0,
                    reward: 1_000,
                    finder: ALICE,
                    found_at_millis: 0,
                    state: BlockState::Immature,
                },
                &[PayoutEntry {
                    miner: ALICE,
                    amount: 1_000,
                }],
            )
            .unwrap();

        let (status, body) = get(state, &format!("/miner/{}", hex::encode(ALICE))).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Immature"));
        assert!(body.contains("1000"));
    }

    #[tokio::test]
    async fn there_is_no_route_that_changes_pool_state() {
        // A dashboard that could act on the pool would be a second and much
        // weaker door into the parts that handle money.
        let state = state();
        for path in ["/", "/api/status", "/payouts"] {
            use tower::ServiceExt;

            let response = router(Arc::clone(&state))
                .oneshot(
                    axum::http::Request::builder()
                        .method("POST")
                        .uri(path)
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(
                response.status(),
                StatusCode::METHOD_NOT_ALLOWED,
                "{path} accepted a POST"
            );
        }
    }

    #[tokio::test]
    async fn healthz_answers_before_any_work_exists() {
        let (status, body) = get(state(), "/healthz").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "ok");
    }

    #[tokio::test]
    async fn a_ledger_failure_is_not_reported_as_the_callers_mistake() {
        // The distinction the error mapping exists for: a 500 sends an operator
        // to the logs, a 400 sends the miner in circles.
        let error = PoolError::Ledger("disk is gone".to_string());
        assert_eq!(
            error.into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
