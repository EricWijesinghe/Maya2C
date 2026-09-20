//! The telemetry collector daemon.
//!
//! ```text
//! MAYA_TELEMETRY_LISTEN=0.0.0.0:9100 maya-telemetry
//! ```
//!
//! # It sweeps on a timer as well as on every report
//!
//! Snapshots are published when a report arrives, which is exactly wrong for
//! the case that matters: a network where everybody stopped reporting produces
//! no reports and therefore no snapshots, and the dashboard would show the
//! last healthy picture indefinitely. The sweep is what makes an empty network
//! look empty.

use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use maya_telemetry::http::{TelemetryService, router};

/// How often expired reports are dropped and a snapshot republished.
///
/// Well under the report TTL, so a reporter that vanishes disappears from the
/// dashboard promptly rather than at the next unrelated report.
const SWEEP_INTERVAL: Duration = Duration::from_secs(15);

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt::init();

    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("telemetry: {message}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let listen: SocketAddr = std::env::var("MAYA_TELEMETRY_LISTEN")
        .unwrap_or_else(|_| "127.0.0.1:9100".to_string())
        .parse()
        .map_err(|e| format!("MAYA_TELEMETRY_LISTEN is not an address: {e}"))?;

    let trust_proxy = std::env::var("MAYA_TELEMETRY_TRUST_PROXY").as_deref() == Ok("true");

    let service = Arc::new(TelemetryService::new().trust_proxy(trust_proxy));

    if trust_proxy {
        tracing::info!(
            header = maya_telemetry::http::COUNTRY_HEADER,
            "reading the country from the proxy header"
        );
    } else {
        tracing::info!(
            "geolocation is off; every reporter will be counted as unknown. Set \
             MAYA_TELEMETRY_TRUST_PROXY=true only when a geolocating proxy is the sole \
             route to this port — otherwise the header is whatever the reporter typed."
        );
    }

    {
        let service = Arc::clone(&service);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(SWEEP_INTERVAL);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                let snapshot = service.sweep(seconds_since_epoch());
                tracing::debug!(
                    reporters = snapshot.reporters,
                    hash_rate = snapshot.hash_rate,
                    "swept"
                );
            }
        });
    }

    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(|e| format!("binding {listen}: {e}"))?;
    tracing::info!(%listen, "listening");

    // The connect info is required by the report handler's extractor. It reads
    // nothing from the address — see `http::report` — but binding it is what
    // makes it structurally impossible to add a handler that silently starts
    // storing one.
    axum::serve(
        listener,
        router(service).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .map_err(|e| format!("serving: {e}"))
}

fn seconds_since_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
