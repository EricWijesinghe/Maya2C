//! `maya2c-agent-registry`: the HTTP registry, bound to loopback by default.
//!
//! SIM / LOCAL-ONLY: records live in memory and vanish on restart. Nothing
//! here is consensus (ADR-130).

use std::net::SocketAddr;

use maya_agent_registry::http::{AppState, router};
use maya_agent_registry::limit::FixedWindow;
use maya_agent_registry::registry::Registry;

/// Per-client budget: requests per window. Chosen for a local service, not measured.
const WINDOW_SECS: u64 = 60;
const MAX_PER_WINDOW: u32 = 120;
const DEFAULT_LISTEN: &str = "127.0.0.1:8790";

#[tokio::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt().init();
    let listen = std::env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_LISTEN.to_owned());
    let addr: SocketAddr = listen
        .parse()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    if !addr.ip().is_loopback() {
        tracing::warn!("binding a non-loopback address: the registry has no TLS and no auth yet");
    }
    tracing::warn!("LOCAL-ONLY: in-memory registry, nothing persists across restarts (ADR-130)");

    let state = AppState::new(
        Registry::default(),
        FixedWindow::new(WINDOW_SECS, MAX_PER_WINDOW),
    );
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("listening on {addr}");
    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
}
