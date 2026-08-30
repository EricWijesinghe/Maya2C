//! The `/metrics` exporter.
//!
//! A deliberately minimal HTTP server. It answers `GET /metrics` and
//! `GET /healthz` and nothing else — every other path is a 404. Prometheus
//! needs no more than that, and an exporter with a wider surface is a wider
//! surface on an endpoint that carries operational data.
//!
//! Hyper is used directly rather than a framework because the node already
//! pulls it in through jsonrpsee, so this costs no new dependency.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use http_body_util::Full;
use hyper::body::Bytes;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

use crate::metrics::Metrics;

/// Content type Prometheus expects for text exposition.
const EXPOSITION_CONTENT_TYPE: &str = "application/openmetrics-text; version=1.0.0; charset=utf-8";

/// A running exporter.
#[derive(Debug)]
pub struct MetricsServer {
    /// Address actually bound, which matters when the caller asked for port 0.
    pub address: SocketAddr,
}

/// Binds the exporter and serves it on a background task.
///
/// # Errors
///
/// Returns an error if the address cannot be bound.
pub async fn serve(address: SocketAddr, metrics: Arc<Metrics>) -> std::io::Result<MetricsServer> {
    let listener = TcpListener::bind(address).await?;
    let bound = listener.local_addr()?;

    tokio::spawn(async move {
        loop {
            let (stream, _peer) = match listener.accept().await {
                Ok(accepted) => accepted,
                // A failed accept is not a reason to stop exporting; the next
                // scrape should still find a listener.
                Err(error) => {
                    eprintln!("metrics: accept failed: {error}");
                    continue;
                }
            };

            let metrics = Arc::clone(&metrics);
            tokio::spawn(async move {
                let service = service_fn(move |request| {
                    let metrics = Arc::clone(&metrics);
                    async move { route(request, metrics) }
                });

                if let Err(error) = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await
                {
                    // Scrapers hang up abruptly all the time; this is noise at
                    // debug level, not an error worth surfacing.
                    let _ = error;
                }
            });
        }
    });

    Ok(MetricsServer { address: bound })
}

/// Routes a single request.
fn route(
    request: Request<hyper::body::Incoming>,
    metrics: Arc<Metrics>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let response = match (request.method(), request.uri().path()) {
        (&hyper::Method::GET, "/metrics") => match metrics.encode() {
            Ok(body) => Response::builder()
                .status(StatusCode::OK)
                .header(hyper::header::CONTENT_TYPE, EXPOSITION_CONTENT_TYPE)
                .body(Full::new(Bytes::from(body))),
            Err(error) => Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Full::new(Bytes::from(format!("encoding failed: {error}")))),
        },
        (&hyper::Method::GET, "/healthz") => Response::builder()
            .status(StatusCode::OK)
            .body(Full::new(Bytes::from("ok"))),
        _ => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Full::new(Bytes::new())),
    };

    Ok(response.unwrap_or_else(|_| {
        // Builder failure means a malformed header we constructed ourselves,
        // which is a bug rather than a request problem — but the exporter must
        // still answer rather than drop the connection.
        let mut fallback = Response::new(Full::new(Bytes::new()));
        *fallback.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
        fallback
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Binds on an ephemeral port so tests can run concurrently.
    async fn start() -> (MetricsServer, Arc<Metrics>) {
        let metrics = Arc::new(Metrics::new());
        let server = serve(
            "127.0.0.1:0".parse().expect("valid address"),
            Arc::clone(&metrics),
        )
        .await
        .expect("bind");
        (server, metrics)
    }

    /// Minimal HTTP/1.1 GET, so the test does not need a client dependency.
    async fn get(address: SocketAddr, path: &str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let mut stream = tokio::net::TcpStream::connect(address)
            .await
            .expect("connect");
        let request =
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).await.expect("write");

        let mut response = String::new();
        stream.read_to_string(&mut response).await.expect("read");
        response
    }

    #[tokio::test]
    async fn metrics_are_served_in_exposition_format() {
        let (server, metrics) = start().await;
        metrics.set_peers(4);

        let response = get(server.address, "/metrics").await;
        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("maya_peers_connected 4"));
        assert!(response.contains("openmetrics-text"));
    }

    #[tokio::test]
    async fn the_exporter_reflects_later_updates() {
        // The handler must read through the shared registry on every scrape,
        // not capture a snapshot at startup.
        let (server, metrics) = start().await;

        metrics.set_height(1);
        assert!(
            get(server.address, "/metrics")
                .await
                .contains("maya_chain_height 1")
        );

        metrics.set_height(99);
        assert!(
            get(server.address, "/metrics")
                .await
                .contains("maya_chain_height 99")
        );
    }

    #[tokio::test]
    async fn healthz_answers() {
        let (server, _metrics) = start().await;
        let response = get(server.address, "/healthz").await;
        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.ends_with("ok"));
    }

    #[tokio::test]
    async fn unknown_paths_are_not_found() {
        let (server, _metrics) = start().await;
        assert!(get(server.address, "/").await.starts_with("HTTP/1.1 404"));
        assert!(
            get(server.address, "/../etc/passwd")
                .await
                .starts_with("HTTP/1.1 404")
        );
    }

    #[tokio::test]
    async fn the_exporter_survives_many_scrapes() {
        // A scraper polls forever; a server that leaked a task or a socket per
        // connection would fail here rather than in production at 3am.
        let (server, metrics) = start().await;
        metrics.set_peers(2);

        for _ in 0..25 {
            assert!(
                get(server.address, "/metrics")
                    .await
                    .contains("maya_peers_connected 2")
            );
        }
    }
}
