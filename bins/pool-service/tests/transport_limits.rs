//! Connection limits at the socket: what the pool does with a peer that never
//! speaks. The transport itself is covered by `src/daemon.rs` and by the
//! simulation tests; these tests are about the time a stranger may hold a slot.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use custom_l1_node::crypto::dag::registry::{CacheRegistry, DagConfig};
use maya_pool_service::config::PoolConfig;
use maya_pool_service::daemon;
use maya_pool_service::ledger::memory::MemoryLedger;
use maya_pool_service::metrics::PoolMetrics;
use maya_pool_service::state::PoolState;
use maya_pool_service::validator::Validator;
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tokio::sync::{broadcast, mpsc};

/// The pool's handshake deadline, plus margin for scheduling.
const DEADLINE: Duration = Duration::from_secs(20);

/// A peer that connects and sends nothing is closed by the pool once the
/// handshake deadline passes. Before the deadline existed, the socket and its
/// task were held until the peer itself gave up.
#[tokio::test]
async fn a_silent_socket_is_closed_after_the_handshake_timeout() {
    let state = Arc::new(PoolState::new(
        PoolConfig::default(),
        Arc::new(MemoryLedger::new()),
        Arc::new(PoolMetrics::new()),
        Arc::new(CacheRegistry::new(DagConfig::TESTING)),
    ));
    let (blocks, _found) = mpsc::channel(4);
    let validator = Validator::spawn(Arc::clone(&state), 1, blocks);
    let (jobs, _) = broadcast::channel(4);
    let server = daemon::serve_stratum("127.0.0.1:0".parse().unwrap(), state, validator, jobs)
        .await
        .unwrap();

    let mut silent = TcpStream::connect(server.address).await.unwrap();
    // The responder speaks first, so the peer reads a little before the pool
    // gives up on it. Reading to end of stream is what proves the close.
    let mut sink = [0u8; 4096];
    let drained = tokio::time::timeout(DEADLINE, async {
        loop {
            match silent.read(&mut sink).await {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
        }
    })
    .await;
    assert!(
        drained.is_ok(),
        "the pool kept a silent socket open past its handshake deadline"
    );
}
