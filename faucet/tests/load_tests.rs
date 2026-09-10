//! A thousand concurrent requests, and what that actually proves.
//!
//! # Read the test names before the numbers
//!
//! It is tempting to call this a Sybil test. It is not one, and saying so
//! would be worse than not testing at all — it would put a name on the
//! deployment page that the code does not earn.
//!
//! A thousand requests from a thousand IPs with a thousand fresh addresses is
//! a thousand requests that are *indistinguishable from a thousand real
//! users*. The limiter grants all of them, and it is right to: nothing in the
//! request says otherwise. What actually bounds that case is the daily cap,
//! which is why [`no_more_than_the_daily_cap_leaves_under_concurrent_load`]
//! exists and why it is the one to read first.
//!
//! So what these tests prove, precisely:
//!
//! - **Under concurrency the limiter still limits.** One IP racing a thousand
//!   requests gets one grant, not a number that depends on scheduling.
//! - **The cap is a cap, not a target.** However many callers arrive at once,
//!   the total dispensed never exceeds it — which requires the budget check and
//!   the grant to be one atomic step.
//! - **A refusal is cheap.** Nine hundred and ninety-nine refusals do not
//!   consume budget, so a flood costs the faucet nothing but CPU.
//!
//! What they do not prove: that a distributed attacker cannot take the day's
//! budget. They can, and the cap is the choice of how much that costs.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use maya_faucet::Faucet;
use maya_faucet::http::{FaucetService, router};
use maya_faucet::testing::RecordingDispenser;
use tower::ServiceExt;

/// Requests fired at once.
const CONCURRENCY: usize = 1_000;

const DISPENSE: u64 = 100;

fn address(index: usize) -> String {
    let mut bytes = [0u8; 32];
    bytes[..8].copy_from_slice(&(index as u64).to_le_bytes());
    hex::encode(bytes)
}

/// A distinct address per index, inside 203.0.113.0/24's worth of space and
/// beyond — `10.x` gives room for a thousand without collisions.
fn peer(index: usize) -> String {
    format!("10.{}.{}.1", index / 256, index % 256)
}

fn grant_request(peer: &str, address: &str) -> Request<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri("/request")
        .header("content-type", "application/json")
        .body(Body::from(format!(r#"{{"address":"{address}"}}"#)))
        .expect("request");

    let peer: SocketAddr = format!("{peer}:40000").parse().expect("peer address");
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(peer));
    request
}

fn app(daily_cap: u64, dispenser: Arc<RecordingDispenser>) -> axum::Router {
    let faucet = Faucet::new(
        "maya-genesis-rc1",
        DISPENSE,
        daily_cap,
        std::time::SystemTime::now(),
    )
    .expect("a testnet chain");
    router(Arc::new(FaucetService::new(faucet, dispenser)))
}

/// Fires every request concurrently and returns the status codes.
async fn fire(app: axum::Router, requests: Vec<Request<Body>>) -> Vec<StatusCode> {
    let handles: Vec<_> = requests
        .into_iter()
        .map(|request| {
            let app = app.clone();
            tokio::spawn(async move { app.oneshot(request).await.expect("served").status() })
        })
        .collect();

    let mut statuses = Vec::with_capacity(handles.len());
    for handle in handles {
        statuses.push(handle.await.expect("no task panicked"));
    }
    statuses
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn one_ip_racing_a_thousand_requests_still_gets_one_grant() {
    // The limiter under contention. Sequentially this is already tested; the
    // question here is whether the check and the commit are one step. If they
    // were not, the number of grants would depend on how the scheduler
    // interleaved a thousand threads — and would be greater than one.
    let dispenser = Arc::new(RecordingDispenser::new());
    let app = app(u64::MAX, Arc::clone(&dispenser));

    let requests = (0..CONCURRENCY)
        .map(|i| grant_request("203.0.113.1", &address(i)))
        .collect();

    let statuses = fire(app, requests).await;

    let granted = statuses.iter().filter(|s| **s == StatusCode::OK).count();
    assert_eq!(granted, 1, "one IP, one grant, regardless of interleaving");
    assert_eq!(
        statuses
            .iter()
            .filter(|s| **s == StatusCode::TOO_MANY_REQUESTS)
            .count(),
        CONCURRENCY - 1
    );
    assert_eq!(dispenser.total(), DISPENSE);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn one_address_racing_a_thousand_ips_still_gets_one_grant() {
    // The mirror image, and the one a botnet would actually attempt against a
    // single wallet.
    let dispenser = Arc::new(RecordingDispenser::new());
    let app = app(u64::MAX, Arc::clone(&dispenser));

    let requests = (0..CONCURRENCY)
        .map(|i| grant_request(&peer(i), &address(0)))
        .collect();

    let statuses = fire(app, requests).await;

    assert_eq!(
        statuses.iter().filter(|s| **s == StatusCode::OK).count(),
        1,
        "one address, one grant"
    );
    assert_eq!(dispenser.total(), DISPENSE);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn no_more_than_the_daily_cap_leaves_under_concurrent_load() {
    // **The control that matters.** A thousand distinct IPs with a thousand
    // distinct addresses is a thousand requests the limiter cannot fault, and
    // it grants every one it is allowed to. The cap is what makes that cost a
    // number somebody chose.
    //
    // A cap of 1,000 units at 100 per grant is exactly ten grants. If the
    // budget check and the grant were not atomic, this would over-dispense
    // under load — the classic read-then-write race, worth a real test rather
    // than a comment.
    const DAILY_CAP: u64 = 1_000;

    let dispenser = Arc::new(RecordingDispenser::new());
    let app = app(DAILY_CAP, Arc::clone(&dispenser));

    let requests = (0..CONCURRENCY)
        .map(|i| grant_request(&peer(i), &address(i)))
        .collect();

    let statuses = fire(app, requests).await;

    let granted = statuses.iter().filter(|s| **s == StatusCode::OK).count();
    assert_eq!(
        granted,
        (DAILY_CAP / DISPENSE) as usize,
        "exactly the cap, not one grant more"
    );
    assert_eq!(
        dispenser.total(),
        DAILY_CAP,
        "the ledger the operator tops up must match the ledger the faucet reports"
    );

    // Everything else is a 503, not a 500. A busy faucet is not a broken one.
    assert_eq!(
        statuses
            .iter()
            .filter(|s| **s == StatusCode::SERVICE_UNAVAILABLE)
            .count(),
        CONCURRENCY - granted
    );
    assert!(
        !statuses.iter().any(StatusCode::is_server_error)
            || statuses
                .iter()
                .all(|s| *s != StatusCode::INTERNAL_SERVER_ERROR),
        "load must not produce internal errors"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_flood_of_refusals_costs_the_faucet_no_budget() {
    // Nine hundred and ninety-nine refusals must leave the budget where a
    // single grant left it. If a refusal consumed anything, a flood would be a
    // denial of service against the users the faucet is for — cheaper than
    // draining it, and just as effective.
    let dispenser = Arc::new(RecordingDispenser::new());
    let app = app(1_000, Arc::clone(&dispenser));

    let requests = (0..CONCURRENCY)
        .map(|i| grant_request("203.0.113.1", &address(i)))
        .collect();

    fire(app.clone(), requests).await;
    assert_eq!(dispenser.total(), DISPENSE, "one grant, nothing else spent");

    // And nine other users can still be funded, which is what "the flood cost
    // nothing" has to mean.
    let requests = (1..=9)
        .map(|i| grant_request(&peer(i), &address(10_000 + i)))
        .collect();
    let statuses = fire(app, requests).await;
    assert!(
        statuses.iter().all(|s| *s == StatusCode::OK),
        "the day's remaining budget must still be available to real users: {statuses:?}"
    );
}
