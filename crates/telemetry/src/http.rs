//! The collector's HTTP surface, including the live WebSocket feed.
//!
//! | Route | Purpose |
//! |---|---|
//! | `POST /report` | a miner or node submits |
//! | `GET /api/snapshot` | the current aggregate, once |
//! | `GET /api/ws` | the same aggregate, pushed on every change |
//! | `GET /health` | liveness |
//!
//! # The live feed broadcasts, it does not poll per client
//!
//! One [`tokio::sync::broadcast`] channel carries snapshots to every connected
//! browser. The alternative — a task per socket rebuilding the snapshot on a
//! timer — makes the collector's work proportional to the number of people
//! looking at the dashboard, which is exactly backwards for a page that gets
//! linked from somewhere busy.
//!
//! A slow client that falls behind is **lagged, not disconnected**. Snapshots
//! are absolute rather than incremental, so a client that missed three of them
//! is fully correct after the fourth; there is no state to rebuild and nothing
//! to resynchronise.
//!
//! # The reporter's address is used once and dropped
//!
//! [`TelemetryService::ingest`] derives a [`Region`] from the geolocation header and never puts
//! the address anywhere else. That is the whole of the privacy design's
//! enforcement — see `region.rs` for why the granularity is what it is.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use tokio::sync::broadcast;

use crate::collector::Collector;
use crate::region::Region;
use crate::report::Report;
use crate::snapshot::Snapshot;

/// Header a geolocating reverse proxy sets to a two-letter country code.
///
/// Cloudflare's name for it, because that is the most common deployment; any
/// proxy can be configured to write the same header.
pub const COUNTRY_HEADER: &str = "cf-ipcountry";

/// Largest report body accepted.
///
/// A report is one small JSON object. Anything larger is not one, and reading
/// it would be free memory for whoever sent it.
pub const MAX_BODY_BYTES: usize = 4096;

/// Snapshots buffered for a client that has fallen behind.
///
/// Small on purpose: snapshots are absolute, so a client that missed several
/// is fully correct after the next one. Buffering more would trade memory for
/// staleness nobody sees.
pub const BROADCAST_CAPACITY: usize = 16;

/// The collector, the broadcast channel, and the proxy-trust setting.
pub struct TelemetryService {
    collector: Mutex<Collector>,
    updates: broadcast::Sender<Snapshot>,
    /// Whether [`COUNTRY_HEADER`] is read.
    ///
    /// Off by default: with no geolocating proxy in front, the header is
    /// whatever the reporter typed, and a map built from it is a map of what
    /// reporters chose to claim.
    trust_proxy: bool,
}

impl TelemetryService {
    /// Builds a service keyed on no geolocation.
    #[must_use]
    pub fn new() -> Self {
        let (updates, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            collector: Mutex::new(Collector::new()),
            updates,
            trust_proxy: false,
        }
    }

    /// Reads the country from [`COUNTRY_HEADER`].
    ///
    /// Only correct when a geolocating proxy is the sole route to this
    /// service.
    #[must_use]
    pub fn trust_proxy(mut self, trust: bool) -> Self {
        self.trust_proxy = trust;
        self
    }

    /// Whether the geolocation header is honoured.
    #[must_use]
    pub const fn trusts_proxy(&self) -> bool {
        self.trust_proxy
    }

    /// Accepts a report and publishes the resulting snapshot.
    ///
    /// The address is taken by value and dropped here. It is used for
    /// nothing but deriving the region, and no caller receives it back.
    ///
    /// # Errors
    ///
    /// The report's own validation error.
    pub fn ingest(
        &self,
        report: Report,
        region: Region,
        now: u64,
    ) -> Result<Snapshot, crate::report::ReportError> {
        let snapshot = {
            let mut collector = self.lock();
            collector.accept(report, region, now)?;
            collector.snapshot(now)
        };

        // An error here means nobody is listening, which is the normal state
        // for a dashboard nobody has open.
        let _ = self.updates.send(snapshot.clone());
        Ok(snapshot)
    }

    /// The current aggregate.
    #[must_use]
    pub fn snapshot(&self, now: u64) -> Snapshot {
        self.lock().snapshot(now)
    }

    /// Drops expired reports and publishes the result.
    ///
    /// Called on a timer by the daemon: without it a dashboard nobody is
    /// reporting to would keep showing the last snapshot forever, since
    /// snapshots are only published when a report arrives.
    pub fn sweep(&self, now: u64) -> Snapshot {
        let snapshot = {
            let mut collector = self.lock();
            collector.sweep(now);
            collector.snapshot(now)
        };
        let _ = self.updates.send(snapshot.clone());
        snapshot
    }

    /// A receiver for the live feed.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Snapshot> {
        self.updates.subscribe()
    }

    /// A poisoned lock means a thread panicked mid-update. Continuing is
    /// right: the map is structurally intact, and the alternative is a
    /// dashboard that goes down because one request panicked.
    fn lock(&self) -> std::sync::MutexGuard<'_, Collector> {
        self.collector
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Default for TelemetryService {
    fn default() -> Self {
        Self::new()
    }
}

/// The routes, ready to serve.
pub fn router(service: Arc<TelemetryService>) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/report", post(report))
        .route("/api/snapshot", get(snapshot))
        .route("/api/ws", get(websocket))
        .layer(tower_http::limit::RequestBodyLimitLayer::new(
            MAX_BODY_BYTES,
        ))
        // The dashboard is served from a different origin than the collector
        // in most deployments. Reads are public data; `POST /report` is not
        // browser-originated, so only GET is allowed cross-origin.
        .layer(
            tower_http::cors::CorsLayer::new()
                .allow_origin(tower_http::cors::Any)
                .allow_methods([axum::http::Method::GET]),
        )
        .with_state(service)
}

/// An error, as a reporter sees it.
#[derive(Debug, serde::Serialize)]
pub struct ErrorBody {
    /// A short machine-readable tag.
    pub error: &'static str,
    /// A sentence naming the field.
    pub message: String,
}

async fn report(
    State(service): State<Arc<TelemetryService>>,
    ConnectInfo(_peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(report): Json<Report>,
) -> Response {
    // The one place an address becomes a region. `_peer` is bound and unused
    // deliberately: the extractor is what makes the connection info available,
    // and naming it here documents that nothing reads it.
    let region = if service.trust_proxy {
        country_from(&headers)
    } else {
        Region::UNKNOWN
    };

    match service.ingest(report, region, now()) {
        Ok(snapshot) => (StatusCode::ACCEPTED, Json(snapshot)).into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(ErrorBody {
                error: "invalid_report",
                message: error.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn snapshot(State(service): State<Arc<TelemetryService>>) -> Json<Snapshot> {
    Json(service.snapshot(now()))
}

async fn websocket(
    State(service): State<Arc<TelemetryService>>,
    upgrade: WebSocketUpgrade,
) -> Response {
    upgrade.on_upgrade(move |socket| feed(socket, service))
}

/// Pushes the current snapshot, then every subsequent one.
///
/// The immediate first send matters: a client that connected between two
/// reports would otherwise stare at an empty page until somebody reported.
async fn feed(mut socket: WebSocket, service: Arc<TelemetryService>) {
    let mut updates = service.subscribe();

    if send(&mut socket, &service.snapshot(now())).await.is_err() {
        return;
    }

    loop {
        match updates.recv().await {
            Ok(snapshot) => {
                if send(&mut socket, &snapshot).await.is_err() {
                    return;
                }
            }
            // Behind, but not broken. Snapshots are absolute, so the next one
            // makes this client correct again — there is nothing to
            // resynchronise and no reason to drop them.
            Err(broadcast::error::RecvError::Lagged(missed)) => {
                tracing::debug!(
                    missed,
                    "a dashboard fell behind; it recovers on the next snapshot"
                );
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

async fn send(socket: &mut WebSocket, snapshot: &Snapshot) -> Result<(), axum::Error> {
    let json = serde_json::to_string(snapshot).map_err(axum::Error::new)?;
    socket.send(Message::Text(json.into())).await
}

/// The country from the proxy's header, or [`Region::UNKNOWN`].
fn country_from(headers: &HeaderMap) -> Region {
    headers
        .get(COUNTRY_HEADER)
        .and_then(|value| value.to_str().ok())
        .map_or(Region::UNKNOWN, Region::parse)
}

/// Seconds since the Unix epoch.
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn a_missing_country_header_is_unknown_not_a_default_country() {
        // Defaulting to any country would put reporters on a map in a place
        // nobody claimed they were.
        assert_eq!(country_from(&HeaderMap::new()), Region::UNKNOWN);
    }

    #[test]
    fn a_country_header_is_parsed() {
        let mut headers = HeaderMap::new();
        headers.insert(COUNTRY_HEADER, HeaderValue::from_static("de"));
        assert_eq!(country_from(&headers), Region::parse("DE"));
    }

    #[test]
    fn geolocation_is_not_trusted_by_default() {
        // With no geolocating proxy, the header is whatever the reporter
        // typed, and the map would be of what reporters chose to claim.
        assert!(!TelemetryService::new().trusts_proxy());
    }

    #[test]
    fn a_service_with_no_listeners_still_accepts_reports() {
        // The normal state for a dashboard nobody has open. A failed
        // broadcast must not fail the report.
        let service = TelemetryService::new();
        let report = Report::Miner(crate::report::MinerReport {
            reporter: crate::report::ReporterId::parse("rig-1").expect("id"),
            hash_rate: 500,
            backend: "wgpu".to_string(),
            device: "test".to_string(),
        });

        let snapshot = service
            .ingest(report, Region::UNKNOWN, 1_000)
            .expect("accepted");
        assert_eq!(snapshot.hash_rate, 500);
    }
}
