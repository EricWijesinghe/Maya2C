//! End-to-end socket tests: a real `TcpListener` on loopback, a real post-quantum
//! handshake from a client, and the pool's own read and write loops. These pin
//! what a stranger can make the pool hold: idle time, unread output, and the
//! authenticated cap. Unit tests in `src/daemon.rs` cover the bookkeeping.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use custom_l1_node::crypto::dag::registry::{CacheRegistry, DagConfig};
use custom_l1_node::network::pq::handshake;
use custom_l1_node::network::pq::stream::PqStream;
use futures::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use maya_pool_service::config::{DEFAULT_MAX_CONNECTIONS_PER_IP, PoolConfig};
use maya_pool_service::daemon;
use maya_pool_service::ledger::memory::MemoryLedger;
use maya_pool_service::metrics::PoolMetrics;
use maya_pool_service::state::PoolState;
use maya_pool_service::validator::Validator;
use maya_stratum_v2::frame::FRAME_HEADER_LEN;
use maya_stratum_v2::messages::common::SetupConnection;
use maya_stratum_v2::{Frame, FrameHeader, Message, Protocol};
use tokio::net::TcpStream;
use tokio::sync::{broadcast, mpsc};
use tokio_util::compat::{Compat, TokioAsyncReadCompatExt};

type Secure = PqStream<Compat<TcpStream>>;

/// Long enough that no test sees the idle close unless it asks for it.
const DEFAULT_IDLE: Duration = Duration::from_secs(300);

/// Upper bound on any single wait in these tests.
const WAIT: Duration = Duration::from_secs(10);

fn fresh_state() -> Arc<PoolState> {
    Arc::new(PoolState::new(
        PoolConfig::default(),
        Arc::new(MemoryLedger::new()),
        Arc::new(PoolMetrics::new()),
        Arc::new(CacheRegistry::new(DagConfig::TESTING)),
    ))
}

/// Binds a pool on loopback with the given per-address cap and idle timeout.
async fn serve(per_ip_cap: usize, idle: Duration) -> SocketAddr {
    let state = fresh_state();
    // No block is found in these tests, so the receiver can go.
    let (blocks, _) = mpsc::channel(4);
    let validator = Validator::spawn(Arc::clone(&state), 1, blocks);
    let (jobs, _) = broadcast::channel(4);
    let address = Ipv4Addr::LOCALHOST.into();
    daemon::serve_stratum_tuned(
        SocketAddr::new(address, 0),
        state,
        validator,
        jobs,
        per_ip_cap,
        idle,
    )
    .await
    .unwrap()
    .address
}

/// Completes the client side of the handshake; the stream is ready for frames.
async fn connect(address: SocketAddr) -> Secure {
    let mut compat = TcpStream::connect(address).await.unwrap().compat();
    let keys = handshake::initiate(&mut compat).await.unwrap();
    PqStream::new(compat, keys, true)
}

fn setup() -> Vec<u8> {
    Message::SetupConnection(SetupConnection {
        protocol: Protocol::Mining,
        min_version: 2,
        max_version: 2,
        flags: 0,
        endpoint_host: "127.0.0.1".to_string(),
        endpoint_port: 0,
        vendor: "socket-e2e".to_string(),
        hardware_version: "test".to_string(),
        firmware: "test".to_string(),
        device_id: "rig".to_string(),
    })
    .to_frame()
    .unwrap()
    .encode()
}

fn io_other(error: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(error.to_string())
}

/// Reads one frame. `None` is a clean end of stream.
async fn read_one<R: AsyncRead + Unpin>(reader: &mut R) -> std::io::Result<Option<Message>> {
    let mut header = [0u8; FRAME_HEADER_LEN];
    match reader.read_exact(&mut header).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let decoded = FrameHeader::decode(&header).map_err(io_other)?;
    let mut whole = header.to_vec();
    whole.resize(FRAME_HEADER_LEN + decoded.msg_length as usize, 0);
    reader.read_exact(&mut whole[FRAME_HEADER_LEN..]).await?;
    let frame = Frame::decode(&whole).map_err(io_other)?;
    Message::from_frame(&frame).map(Some).map_err(io_other)
}

/// Connects, sends `SetupConnection`, and returns once the pool has answered.
/// The answer is read only after the pool admitted the connection, so the
/// connection is in the authenticated registry when this returns.
async fn admitted(address: SocketAddr) -> Secure {
    let mut secure = connect(address).await;
    secure.write_all(&setup()).await.unwrap();
    secure.flush().await.unwrap();
    let reply = tokio::time::timeout(WAIT, read_one(&mut secure))
        .await
        .expect("the pool answered setup")
        .unwrap();
    assert!(matches!(reply, Some(Message::SetupConnectionSuccess(_))));
    secure
}

/// Reads until the pool closes the connection, or fails the test at `WAIT`.
async fn assert_closed_by_pool<R: AsyncRead + Unpin>(reader: &mut R, what: &str) {
    let closed = tokio::time::timeout(WAIT, async {
        loop {
            match read_one(reader).await {
                Ok(None) | Err(_) => return,
                Ok(Some(_)) => {}
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "the pool kept {what} open");
}

#[tokio::test]
async fn an_authenticated_miner_silent_past_the_idle_timeout_is_closed() {
    let address = serve(DEFAULT_MAX_CONNECTIONS_PER_IP, Duration::from_secs(1)).await;
    let mut miner = admitted(address).await;

    assert_closed_by_pool(&mut miner, "a silent miner past its idle timeout").await;
}

#[tokio::test]
async fn a_miner_that_never_reads_is_disconnected_and_the_pool_does_not_buffer_its_replies() {
    // Every SetupConnection draws a reply, so the miner's unread replies would
    // pile up on the pool if nothing bounded them. The outbound queue holds 64
    // messages; past that the pool gives up on the socket. The test does not
    // measure memory: it proves the socket is closed, which is the bound.
    let address = serve(DEFAULT_MAX_CONNECTIONS_PER_IP, DEFAULT_IDLE).await;
    let miner = connect(address).await;
    let frame = setup();

    let started = Instant::now();
    // The read half stays alive, unread, for the whole test.
    let (_unread, mut writer) = miner.split();
    let flood = async {
        loop {
            if writer.write_all(&frame).await.is_err() {
                return;
            }
        }
    };
    let ended = tokio::time::timeout(Duration::from_secs(90), flood).await;
    assert!(
        ended.is_ok(),
        "the pool kept a miner that never reads its replies connected"
    );
    println!(
        "slow reader: pool closed the socket after {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn at_the_cap_the_oldest_authenticated_miner_gives_way_to_a_newcomer() {
    let address = serve(2, DEFAULT_IDLE).await;
    let mut first = admitted(address).await;
    let mut second = admitted(address).await;
    // At the cap of two, admitting this one evicts `first`.
    let mut third = admitted(address).await;

    assert_closed_by_pool(&mut first, "the evicted oldest miner").await;

    for newer in [&mut second, &mut third] {
        newer.write_all(&setup()).await.unwrap();
        newer.flush().await.unwrap();
        let reply = tokio::time::timeout(WAIT, read_one(newer))
            .await
            .expect("a newer miner is still served")
            .unwrap();
        assert!(matches!(reply, Some(Message::SetupConnectionSuccess(_))));
    }
}

/// A listener that never accepts still takes connects into its kernel queue,
/// up to its backlog. Measures how many complete per backlog on this host.
/// Run with `--ignored --nocapture`; the numbers are the measurement.
#[test]
#[ignore = "measurement, not a gate: connects a listener that never accepts"]
fn kernel_backlog_limits_connects_to_a_listener_that_never_accepts() {
    // Negative is Winsock's SOMAXCONN_HINT: request that many, clamped to [200, 65535].
    for backlog in [128, 200, 1_024, 4_096, -4_096] {
        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::STREAM,
            Some(socket2::Protocol::TCP),
        )
        .unwrap();
        let bind: SocketAddr = "127.0.0.1:0".parse().unwrap();
        socket.bind(&bind.into()).unwrap();
        socket.listen(backlog).unwrap();
        let address = socket.local_addr().unwrap().as_socket().unwrap();

        let mut connected = Vec::new();
        let mut failures = std::collections::BTreeMap::<String, usize>::new();
        for _ in 0..1_000 {
            match std::net::TcpStream::connect_timeout(&address, Duration::from_millis(100)) {
                Ok(stream) => connected.push(stream),
                Err(error) => {
                    *failures.entry(format!("{:?}", error.kind())).or_default() += 1;
                }
            }
        }
        println!(
            "listen({backlog}): {} of 1000 connects completed with no accept; failures {failures:?}",
            connected.len()
        );
    }
}

/// The pool, not the kernel, is what is measured here: ten thousand unpaced
/// connects from loopback against the real listener with its accept loop running
/// on a second worker thread.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "measurement, not a gate: 10,000 unpaced connects on loopback"]
async fn ten_thousand_unpaced_connects_all_complete() {
    let address = serve(DEFAULT_MAX_CONNECTIONS_PER_IP, DEFAULT_IDLE).await;
    let mut held = Vec::with_capacity(10_000);
    let mut failures = std::collections::BTreeMap::<String, usize>::new();
    for _ in 0..10_000 {
        match TcpStream::connect(address).await {
            Ok(stream) => held.push(stream),
            Err(error) => {
                *failures.entry(format!("{:?} {:?}", error.kind(), error.raw_os_error())).or_default() += 1;
            }
        }
    }
    println!("10,000 unpaced connects: {} completed; failures {failures:?}", held.len());
    assert!(failures.is_empty(), "connects refused: {failures:?}");
}
