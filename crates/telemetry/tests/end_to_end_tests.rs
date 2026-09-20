//! A real miner reporting to a real collector, over a real socket.
//!
//! The unit tests check each half against its own idea of the wire format.
//! These check the two halves against each other, which is the only thing that
//! catches a serde tag rename — the failure mode where every deployed miner
//! quietly stops being counted and nothing errors anywhere.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use maya_telemetry::http::{TelemetryService, router};
use maya_telemetry::meter::HashMeter;
use maya_telemetry::report::{Report, ReporterId};
use maya_telemetry::reporter::Reporter;

/// Starts the collector on an ephemeral port.
async fn serve(service: Arc<TelemetryService>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("local address");

    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router(service).into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await;
    });

    format!("http://{address}")
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[tokio::test]
async fn a_miners_report_reaches_the_collectors_snapshot() {
    // The whole path: meter -> reporter -> JSON -> socket -> collector ->
    // snapshot. A tag rename on either side breaks exactly here.
    let service = Arc::new(TelemetryService::new());
    let base = serve(Arc::clone(&service)).await;

    let meter = Arc::new(HashMeter::starting_at(Instant::now()));
    meter.record(120_000);

    let reporter = Reporter::new(
        format!("{base}/report"),
        "rig-1",
        "wgpu",
        "test adapter",
        Arc::clone(&meter),
    )
    .expect("valid");

    // Post once directly rather than waiting a minute for the interval.
    let response = post(&format!("{base}/report"), &reporter.current()).await;
    assert!(
        response.is_success(),
        "the collector must accept what the miner sends: {response:?}"
    );

    let snapshot = service.snapshot(now());
    assert_eq!(snapshot.miners, 1);
    assert_eq!(snapshot.reporters, 1);
    assert!(
        snapshot.hash_rate > 0,
        "the meter's rate must survive the round trip"
    );
    assert_eq!(snapshot.backends.len(), 1);
    assert_eq!(snapshot.backends[0].backend, "wgpu");
}

#[tokio::test]
async fn a_dashboard_receives_the_snapshot_the_moment_it_connects() {
    // A client that connected between two reports must not stare at an empty
    // page until somebody reports.
    let service = Arc::new(TelemetryService::new());

    service
        .ingest(
            Report::Miner(maya_telemetry::report::MinerReport {
                reporter: ReporterId::parse("rig-1").expect("id"),
                hash_rate: 5_000,
                backend: "cuda".to_string(),
                device: "test".to_string(),
            }),
            maya_telemetry::region::Region::parse("US"),
            now(),
        )
        .expect("accepted");

    // Subscribing after the fact yields nothing on its own; the handler sends
    // the current snapshot first, which is what this asserts is available.
    let mut updates = service.subscribe();
    assert!(
        updates.try_recv().is_err(),
        "a late subscriber gets no history, which is why the handler sends the current \
         snapshot before entering its loop"
    );
    assert_eq!(service.snapshot(now()).hash_rate, 5_000);
}

#[tokio::test]
async fn every_subscriber_sees_the_next_report() {
    let service = Arc::new(TelemetryService::new());
    let mut first = service.subscribe();
    let mut second = service.subscribe();

    service
        .ingest(
            Report::Miner(maya_telemetry::report::MinerReport {
                reporter: ReporterId::parse("rig-1").expect("id"),
                hash_rate: 7_000,
                backend: "wgpu".to_string(),
                device: "test".to_string(),
            }),
            maya_telemetry::region::Region::UNKNOWN,
            now(),
        )
        .expect("accepted");

    for receiver in [&mut first, &mut second] {
        let snapshot = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("a snapshot within a second")
            .expect("the channel is open");
        assert_eq!(snapshot.hash_rate, 7_000);
    }
}

#[tokio::test]
async fn a_report_the_collector_rejects_is_a_400_and_not_a_500() {
    // A malformed report is the reporter's problem. A 500 would page whoever
    // runs the collector for somebody else's typo.
    let service = Arc::new(TelemetryService::new());
    let base = serve(service).await;

    let bad = serde_json::json!({
        "kind": "miner",
        "reporter": "rig-1",
        "hash_rate": u64::MAX,
        "backend": "wgpu",
        "device": "test",
    });

    let status = post_json(&format!("{base}/report"), &bad).await;
    assert_eq!(
        status.as_u16(),
        400,
        "an implausible claim is a bad request"
    );
}

#[tokio::test]
async fn the_snapshot_endpoint_never_carries_an_address() {
    // The privacy property, checked against the bytes rather than against the
    // type — a field added to `Stored` later would show up right here.
    //
    // Five node reporters, not one, because `MIN_REPORTERS` folds a country
    // with fewer into `ZZ`. That is the floor doing its job, and it is worth
    // seeing the named country in this test so the assertion is about what is
    // published rather than about what was suppressed.
    let service = Arc::new(TelemetryService::new());
    for index in 0..maya_telemetry::region::MIN_REPORTERS {
        service
            .ingest(
                Report::Node(maya_telemetry::report::NodeReport {
                    reporter: ReporterId::parse(&format!("node-{index}")).expect("id"),
                    height: 4_200,
                    peers: 12,
                    propagation_ms: Some(950),
                    client: "maya/0.1.0".to_string(),
                }),
                maya_telemetry::region::Region::parse("DE"),
                now(),
            )
            .expect("accepted");
    }

    let base = serve(Arc::clone(&service)).await;
    let body = get(&format!("{base}/api/snapshot")).await;

    assert!(body.contains("\"height\":4200"), "{body}");
    assert!(body.contains("\"DE\""), "{body}");

    // Exact tokens rather than substrings: a loose `"ip"` would match any
    // future field spelled `description` and turn a real guarantee into a test
    // that fails for the wrong reason.
    for forbidden in [
        "\"ip\"",
        "\"address\"",
        "latitude",
        "longitude",
        "\"lat\"",
        "\"lon\"",
        "127.0.0.1",
    ] {
        assert!(
            !body.contains(forbidden),
            "a public snapshot must not carry {forbidden}: {body}"
        );
    }
}

#[tokio::test]
async fn a_lone_reporter_is_folded_out_of_the_published_map() {
    // The other side of the same property. One node in a country is an
    // individual, and the published bytes must not name where they are.
    let service = Arc::new(TelemetryService::new());
    service
        .ingest(
            Report::Node(maya_telemetry::report::NodeReport {
                reporter: ReporterId::parse("node-1").expect("id"),
                height: 4_200,
                peers: 12,
                propagation_ms: Some(950),
                client: "maya/0.1.0".to_string(),
            }),
            maya_telemetry::region::Region::parse("LI"),
            now(),
        )
        .expect("accepted");

    let base = serve(Arc::clone(&service)).await;
    let body = get(&format!("{base}/api/snapshot")).await;

    assert!(
        !body.contains("\"LI\""),
        "the lone reporter's country must not be published: {body}"
    );
    assert!(
        body.contains("\"ZZ\""),
        "but their contribution is still counted: {body}"
    );
}

// --- a minimal HTTP client, so these tests add no dependency -----------------

use http_body_util::BodyExt;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;

async fn post(url: &str, report: &Report) -> hyper::StatusCode {
    post_json(url, &serde_json::to_value(report).expect("serialize")).await
}

async fn post_json(url: &str, value: &serde_json::Value) -> hyper::StatusCode {
    let client: Client<_, String> = Client::builder(TokioExecutor::new()).build_http();
    let request = hyper::Request::builder()
        .method(hyper::Method::POST)
        .uri(url)
        .header(hyper::header::CONTENT_TYPE, "application/json")
        .body(value.to_string())
        .expect("request");

    client.request(request).await.expect("served").status()
}

async fn get(url: &str) -> String {
    let client: Client<_, String> = Client::builder(TokioExecutor::new()).build_http();
    let request = hyper::Request::builder()
        .uri(url)
        .body(String::new())
        .expect("request");

    let response = client.request(request).await.expect("served");
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    String::from_utf8(bytes.to_vec()).expect("the collector answers in UTF-8")
}
