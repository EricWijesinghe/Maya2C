//! The daemon: the Stratum listener, and the loops around it.
//!
//! ## Transport
//!
//! The pool reuses `src/network/pq` — Noise-style XX over X25519 with an
//! ML-KEM-768 exchange layered inside, `ChaCha20Poly1305`, 64 KiB frames.
//! `docs/stratum-v2.md` §6 gives the reasoning: SV2 specifies
//! `Noise_NX_secp256k1_…`, and adopting it would introduce secp256k1 — the one
//! classical primitive this codebase deliberately avoids — to buy interop that
//! is unreachable anyway.
//!
//! `pq::handshake::respond` is generic over *futures* `AsyncRead`/`AsyncWrite`
//! and a Tokio `TcpStream` implements Tokio's, which is the entire reason
//! `tokio-util`'s compat layer is a dependency.
//!
//! ## One task per connection, and no `select!` over the reader
//!
//! Job pushes arrive on their own task and leave through an outbound queue, so
//! the read loop never sits in a `select!` alongside a partially consumed
//! frame. A cancelled frame read would discard bytes already taken off the
//! socket and desynchronise the stream — a failure that appears as "this one
//! miner sends garbage" long after the cause.
//!
//! ## Verification happens inline, per connection
//!
//! The read loop awaits the validator rather than spawning. That serialises one
//! connection's shares, which is the correct backpressure: vardiff is holding
//! each channel near one share a minute, so a rig waiting on its own share is
//! waiting on work it just asked for. Spawning per share would let one
//! connection queue thousands of verifications and starve every other.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use futures::{AsyncReadExt, AsyncWriteExt};
use maya_stratum_v2::{Frame, FrameHeader, MAX_PAYLOAD_LEN, Message};
use tokio::net::{TcpSocket, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, broadcast, mpsc, oneshot};
use tokio_util::compat::TokioAsyncReadCompatExt;

use custom_l1_node::network::pq::handshake;
use custom_l1_node::network::pq::stream::PqStream;

use crate::error::{PoolError, Result};
use crate::job::Job;
use crate::model::{RejectReason, ShareOutcome, now_millis};
use crate::node::NodeClient;
use crate::payout::PayoutEngine;
use crate::session::{
    InvalidShareBudget, MAX_CHANNELS_PER_CONNECTION, Session, SessionAction, close_message,
    job_messages,
};
use crate::share;
use crate::state::{PoolEvent, PoolState};
use crate::validator::{FoundBlockShare, Validator};

/// Messages queued for one connection before it is considered wedged.
///
/// Four messages per allowed channel: room for a job push to every channel plus
/// the replies in flight. A queue that stays full past `SEND_TIMEOUT` is a
/// socket that has stopped draining, and holding more for it would be holding
/// memory on the pool's behalf rather than theirs.
const OUTBOUND_CAPACITY: usize = MAX_CHANNELS_PER_CONNECTION * 4;

/// Longest a reply may wait for room in a full outbound queue.
///
/// A healthy miner's queue fills during a job-push burst (two messages per
/// channel per job, plus replies) and drains in milliseconds; a short wait
/// rides that out. A reader that never drains is closed after this.
/// shortcut: fixed bound, not measured against real burst sizes, revisit with stratum load data
const SEND_TIMEOUT: Duration = Duration::from_secs(2);

/// Longest a TCP peer may take to finish the post-quantum handshake, end to end.
///
/// The hard cap on the whole responder handshake: its network waits and its
/// budget waits together. Without it, a socket that connects and says nothing
/// holds its task and buffers for as long as the peer keeps the TCP connection
/// open, and the per-step bounds alone add up to more than any honest peer needs.
/// 15 s: above the two 5 s network waits plus CPU, so a slow mobile or satellite
/// link that meets each step still completes.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

/// Handshakes in progress at once, across every connection.
///
/// Each handshake spends CPU on X25519 and ML-KEM on a runtime worker. Capping
/// it apart from connections means a flood of sockets that connect and stall
/// cannot take every worker, while a burst of honest reconnects still gets
/// through a few hundred at a time.
const MAX_CONCURRENT_HANDSHAKES: usize = 256;

/// Longest a connected miner may go without sending a frame.
///
/// Vardiff holds a working rig near one share a minute, so five minutes of
/// silence is a rig that has stopped. Its channel and nonce range go back to
/// the pool rather than sitting reserved.
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// Longest a frame's payload may take to arrive once its header has.
///
/// `IDLE_TIMEOUT` covers waiting for a frame to start. Without this, a peer
/// that sends a valid header and then stops would hold its connection for the
/// full idle window.
const FRAME_TIMEOUT: Duration = Duration::from_secs(10);

/// Queues a reply, waiting at most `timeout` for room. `false` means the queue
/// stayed full or its writer is gone, and the connection should close.
async fn enqueue(outbound: &mpsc::Sender<Message>, message: Message, timeout: Duration) -> bool {
    outbound.send_timeout(message, timeout).await.is_ok()
}

/// Longest one frame may take to leave the socket before the write side gives up.
///
/// A peer that stops reading holds `write_all` forever otherwise, and with it
/// the connection's admission permit.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// Connections served at once. `docs/stratum-v2.md` sizes the pool for 50,000.
const MAX_CONNECTIONS: usize = 50_000;

/// Connections one peer address may hold before its handshake completes.
///
/// Counts from accept until the handshake finishes. This bounds the handshakes
/// one address can run at once, and a neighbour on a shared address can hold at
/// most this many pre-auth slots. Authenticated connections are counted
/// separately, against the operator's cap.
const PREAUTH_PER_IP: usize = 8;

/// Longest each network step of the handshake may wait on the peer.
///
/// The responder speaks first: it sends its key, then waits for the initiator's
/// message. Both waits are network waits and hold no shared budget. Five seconds
/// is sized for slow mobile and satellite links, where a round trip alone can
/// take most of a second. Each wait is also capped by `HANDSHAKE_TIMEOUT`.
const HANDSHAKE_IO_TIMEOUT: Duration = Duration::from_secs(5);

/// Connects one address may open per burst.
///
/// NAT and farm bursts are real: a site that restarts opens its whole fleet at
/// once. The burst is sized for that, and the sustained rate below is what stops
/// a churning peer from tying up handshakes.
const CONNECT_BURST: u32 = 16;

/// Connects one address may open per second, sustained.
///
/// Honest rigs connect once and then stay. Four a second per address still
/// allows a whole site to reconnect after a network blip. Churn faster than that
/// is what fills the pre-auth budget, so the excess is refused at accept.
const CONNECT_RATE_PER_SEC: u64 = 4;

/// Milli-tokens for one connect. Integer arithmetic keeps the bucket exact.
const ONE_CONNECT: u64 = 1_000;

/// Buckets held before full ones are pruned. A full bucket carries no state.
const BUCKET_PRUNE_AT: usize = 10_000;

/// The per-address key for every per-address budget.
///
/// IPv4 is keyed on the full address. IPv6 is keyed on its /64: one host can
/// hold a whole /64, so keying on the full address would let a single subscriber
/// take a fresh budget with every new address. A /64 is the smallest unit a
/// subscriber normally owns, so it is the unit this budget tracks.
// shortcut: /64 is the fixed unit; a provider that delegates smaller prefixes to a single customer needs a shorter key.
#[must_use]
fn address_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => {
            let network = u128::from(v6) & (u128::MAX << 64);
            IpAddr::V6(Ipv6Addr::from(network))
        }
    }
}

/// One address's connect allowance, in milli-tokens, and when it was last topped up.
#[derive(Clone, Copy, Debug)]
struct Bucket {
    tokens: u64,
    last: Instant,
}

/// Milli-tokens an idle bucket has regained by `now`, capped at a full bucket.
fn refilled(bucket: &Bucket, now: Instant) -> u64 {
    let capacity = u64::from(CONNECT_BURST) * ONE_CONNECT;
    let elapsed_ms =
        u64::try_from(now.saturating_duration_since(bucket.last).as_millis()).unwrap_or(u64::MAX);
    // Milli-tokens per millisecond: rate tokens/s is rate*1000 milli-tokens over 1000 ms.
    let regained = elapsed_ms.saturating_mul(CONNECT_RATE_PER_SEC);
    bucket.tokens.saturating_add(regained).min(capacity)
}

/// Takes one connect from `key`'s bucket. `false` means the address is over its rate.
fn take_connect_token(buckets: &mut HashMap<IpAddr, Bucket>, key: IpAddr, now: Instant) -> bool {
    let capacity = u64::from(CONNECT_BURST) * ONE_CONNECT;
    if buckets.len() >= BUCKET_PRUNE_AT {
        buckets.retain(|_, bucket| refilled(bucket, now) < capacity);
    }

    let bucket = buckets.entry(key).or_insert(Bucket {
        tokens: capacity,
        last: now,
    });
    bucket.tokens = refilled(bucket, now);
    bucket.last = now;
    if bucket.tokens >= ONE_CONNECT {
        bucket.tokens -= ONE_CONNECT;
        true
    } else {
        false
    }
}

/// Shortest gap between two lines reporting refused or closed connections.
///
/// A flood refuses thousands of sockets a second; one line per refusal is a
/// synchronous write per stranger, so events are counted and reported in
/// batches instead.
const REFUSAL_LOG_INTERVAL: Duration = Duration::from_secs(10);

/// Open connections per peer address, for one of the two per-address budgets.
///
/// Keyed by full IP address, so an IPv6 peer can rotate within a /64 to get
/// fresh budgets.
// shortcut: full-address keys; upgrade to a prefix key if /64 rotation is seen.
type PerIpCounts = Arc<Mutex<HashMap<IpAddr, usize>>>;

/// One connection's claim on a per-address budget, released when it is dropped.
///
/// Held by the connection task, so every way that task can end (a clean close,
/// an error, a panic unwinding, an abort) gives the slot back.
struct IpSlot {
    counts: PerIpCounts,
    ip: IpAddr,
}

impl IpSlot {
    /// Claims a slot for `ip`, or `None` if that address already has `cap` open.
    fn claim(counts: &PerIpCounts, ip: IpAddr, cap: usize) -> Option<Self> {
        let mut map = counts.lock().unwrap_or_else(PoisonError::into_inner);
        let open = map.entry(ip).or_insert(0);
        if *open >= cap {
            return None;
        }
        *open += 1;
        Some(Self {
            counts: Arc::clone(counts),
            ip,
        })
    }
}

impl Drop for IpSlot {
    fn drop(&mut self) {
        let mut map = self.counts.lock().unwrap_or_else(PoisonError::into_inner);
        let remaining = match map.get_mut(&self.ip) {
            Some(open) => {
                *open = open.saturating_sub(1);
                *open
            }
            None => return,
        };
        if remaining == 0 {
            map.remove(&self.ip);
        }
    }
}

/// Counts refused or closed connections and decides when the count is worth a line.
#[derive(Debug, Default)]
struct ThrottledLog {
    /// When a line was last emitted, if ever.
    last_reported: Option<Instant>,
    /// Events since that line.
    pending: u64,
}

impl ThrottledLog {
    /// Records one event. `Some(count)` means a line is due and reports `count`.
    fn record(&mut self, now: Instant) -> Option<u64> {
        self.pending = self.pending.saturating_add(1);
        let due = match self.last_reported {
            None => true,
            Some(last) => now.saturating_duration_since(last) >= REFUSAL_LOG_INTERVAL,
        };
        if !due {
            return None;
        }
        self.last_reported = Some(now);
        Some(std::mem::take(&mut self.pending))
    }
}

/// Connections the kernel queues before it refuses new ones.
///
/// Tokio's default backlog refused connects in bursts (`os error 10061` in
/// `pool-load` with 10,000 miners). A burst is ordinary when miners reconnect
/// after a restart, so the queue is sized for it.
const LISTEN_BACKLOG: u32 = 4_096;

/// Accepted shares acknowledged in one `SubmitSharesSuccess`.
///
/// SV2 acknowledges in bulk on purpose: at 50,000 connections a reply per
/// submission doubles the pool's write syscalls to carry no information the
/// miner did not already have. Eight is short enough that a rig sees progress
/// within a few minutes at the pool's target share interval.
const ACK_BATCH: u32 = 8;

/// How often the pool asks the node for fresh work.
///
/// Well under the 15-second block target: `docs/stratum-v2.md` §4 requires
/// sub-second job push, and half a second of polling plus the push itself
/// keeps the stale window in the hundreds of milliseconds.
const JOB_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// How often the payout engine runs.
///
/// Payouts are not urgent — they are already waiting on tens of confirmations —
/// and each pass makes several RPC calls.
const SETTLE_INTERVAL: Duration = Duration::from_secs(30);

/// How often gauges are resampled and housekeeping runs.
///
/// Matches the node's own five-second cadence, comfortably under a fifteen
/// second scrape.
const MAINTENANCE_INTERVAL: Duration = Duration::from_secs(5);

/// Share records kept behind the current PPLNS window before pruning.
///
/// Four windows. Deep enough that a burst of found blocks cannot reach past the
/// pruned edge, shallow enough that the ledger does not grow without bound.
const PRUNE_WINDOW_MULTIPLE: u64 = 4;

/// A running pool.
#[derive(Debug)]
pub struct PoolServer {
    /// Address actually bound, which matters when the caller asked for port 0.
    pub address: SocketAddr,
}

/// Pre-auth connections per address, oldest first, each with the signal that evicts it.
#[derive(Default)]
struct PreAuthRegistry {
    by_ip: HashMap<IpAddr, VecDeque<(u64, oneshot::Sender<()>)>>,
    next_id: u64,
}

impl PreAuthRegistry {
    /// Admits one pre-auth connection. At `cap`, the address's oldest pre-auth
    /// connection is evicted first, so a newcomer is never the one refused.
    ///
    /// Returns the new entry's id, its eviction signal, and whether an older
    /// connection was evicted to make room.
    fn admit(&mut self, ip: IpAddr, cap: usize) -> (u64, oneshot::Receiver<()>, bool) {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let (evict, evicted) = oneshot::channel();

        let queue = self.by_ip.entry(ip).or_default();
        let mut evicted_any = false;
        while queue.len() >= cap {
            let Some((_, oldest)) = queue.pop_front() else {
                break;
            };
            // The connection may already be gone; that is fine, its slot is free.
            let _ = oldest.send(());
            evicted_any = true;
        }
        queue.push_back((id, evict));
        (id, evicted, evicted_any)
    }

    /// Forgets one entry. Idempotent: an evicted entry is already gone.
    fn release(&mut self, ip: IpAddr, id: u64) {
        let empty = match self.by_ip.get_mut(&ip) {
            Some(queue) => {
                queue.retain(|(entry, _)| *entry != id);
                queue.is_empty()
            }
            None => false,
        };
        if empty {
            self.by_ip.remove(&ip);
        }
    }
}

type PreAuth = Arc<Mutex<PreAuthRegistry>>;

/// One pre-auth connection's entry, removed when it is dropped.
struct PreAuthSlot {
    registry: PreAuth,
    ip: IpAddr,
    id: u64,
}

impl Drop for PreAuthSlot {
    fn drop(&mut self) {
        let mut registry = self.registry.lock().unwrap_or_else(PoisonError::into_inner);
        registry.release(self.ip, self.id);
    }
}

/// Admission state shared by the accept loop and every connection task.
#[derive(Clone)]
struct Gates {
    /// Synchronous handshake steps running at once, across every connection.
    handshakes: Arc<Semaphore>,
    /// Connect allowances per address, keyed by [`address_key`].
    connect_rate: Arc<Mutex<HashMap<IpAddr, Bucket>>>,
    /// Pre-auth connections per address, for eviction.
    preauth: PreAuth,
    /// Authenticated connections per peer address.
    authenticated: PerIpCounts,
    /// Cap on `authenticated`, per address.
    per_ip_cap: usize,
    /// Throttle for the refused-or-closed log line.
    log: Arc<Mutex<ThrottledLog>>,
}

impl Gates {
    fn new(per_ip_cap: usize) -> Self {
        Self {
            handshakes: Arc::new(Semaphore::new(MAX_CONCURRENT_HANDSHAKES)),
            connect_rate: Arc::new(Mutex::new(HashMap::new())),
            preauth: Arc::new(Mutex::new(PreAuthRegistry::default())),
            authenticated: Arc::new(Mutex::new(HashMap::new())),
            per_ip_cap,
            log: Arc::new(Mutex::new(ThrottledLog::default())),
        }
    }
}

/// Counts one refused or closed connection, and prints a summary when one is due.
fn report_closed(log: &Mutex<ThrottledLog>, peer: SocketAddr, reason: &str) {
    let due = log
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .record(Instant::now());
    if let Some(count) = due {
        eprintln!(
            "pool: {count} connection(s) refused or closed since the last report; \
             latest from {peer}: {reason}"
        );
    }
}

/// Binds the Stratum listener and serves it on a background task.
///
/// # Errors
///
/// Returns [`PoolError::Server`] if the address cannot be bound.
pub async fn serve_stratum(
    address: SocketAddr,
    state: Arc<PoolState>,
    validator: Validator,
    jobs: broadcast::Sender<Arc<Job>>,
) -> Result<PoolServer> {
    let cap = state.config.max_connections_per_ip;
    serve_stratum_capped(address, state, validator, jobs, cap).await
}

/// [`serve_stratum`] with the per-address connection cap set by the caller.
///
/// The load generator runs every simulated miner from loopback, which the
/// production cap would refuse; it passes `usize::MAX` and says so. Production
/// callers use [`serve_stratum`].
///
/// # Errors
///
/// Returns [`PoolError::Server`] if the address cannot be bound.
// Only the pool-load bench may call this. Production goes through `serve_stratum`,
// which applies the operator's configured cap.
#[doc(hidden)]
pub async fn serve_stratum_capped(
    address: SocketAddr,
    state: Arc<PoolState>,
    validator: Validator,
    jobs: broadcast::Sender<Arc<Job>>,
    per_ip_cap: usize,
) -> Result<PoolServer> {
    let socket = if address.is_ipv4() {
        TcpSocket::new_v4()?
    } else {
        TcpSocket::new_v6()?
    };
    socket.bind(address)?;
    let listener = socket.listen(LISTEN_BACKLOG)?;
    let bound = listener.local_addr()?;
    let admission = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let gates = Gates::new(per_ip_cap);

    tokio::spawn(async move {
        loop {
            let (stream, peer) = match listener.accept().await {
                Ok(accepted) => accepted,
                // A failed accept is not a reason to stop listening; the next
                // miner should still find a socket. Same reasoning as the
                // node's metrics exporter.
                Err(error) => {
                    eprintln!("pool: accept failed: {error}");
                    continue;
                }
            };

            // Refused, not queued: a queue here is memory the pool holds on a
            // stranger's behalf. Dropping the socket is the refusal.
            // A connection flood from rotating addresses can still fill the shared
            // admission pool. Per-address budgets cannot stop that, because each
            // address is new; upstream DDoS protection is the mitigation for it.
            let Ok(permit) = Arc::clone(&admission).try_acquire_owned() else {
                report_closed(&gates.log, peer, "connection limit reached");
                continue;
            };

            // Churn faster than handshakes finish is refused here, before any
            // pre-auth entry or handshake work is spent on it.
            let key = address_key(peer.ip());
            let allowed = {
                let mut buckets = gates
                    .connect_rate
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                take_connect_token(&mut buckets, key, Instant::now())
            };
            if !allowed {
                report_closed(&gates.log, peer, "connect rate exceeded for this address");
                continue;
            }

            // At the address's pre-auth cap the oldest pre-auth connection gives
            // way, so a newcomer from a shared address is not the one refused.
            let (id, evicted, evicted_any) = {
                let mut registry = gates.preauth.lock().unwrap_or_else(PoisonError::into_inner);
                registry.admit(key, PREAUTH_PER_IP)
            };
            if evicted_any {
                report_closed(&gates.log, peer, "evicted an older pre-auth connection");
            }
            let pre_slot = PreAuthSlot {
                registry: Arc::clone(&gates.preauth),
                ip: key,
                id,
            };

            let state = Arc::clone(&state);
            let validator = validator.clone();
            let jobs = jobs.subscribe();
            let gates = gates.clone();

            tokio::spawn(async move {
                // Held for the connection's life, released when it ends.
                let _permit = permit;
                let admitted = (pre_slot, evicted);
                if let Err(error) =
                    serve_connection(stream, peer, state, validator, jobs, gates, admitted).await
                {
                    // Connection-level failures are ordinary: miners disconnect,
                    // Ð¸ a malformed frame is a client bug, not a pool one.
                    eprintln!("pool: connection {peer} ended: {error}");
                }
            });
        }
    });

    Ok(PoolServer { address: bound })
}

/// Takes a handshake slot, waiting at most `wait` for one.
///
/// Past `wait` the connection is refused rather than queued: a queue of waiting
/// sockets would hold each one for as long as the flood lasted.
async fn take_handshake_slot(
    slots: Arc<Semaphore>,
    wait: Duration,
) -> Result<OwnedSemaphorePermit> {
    tokio::time::timeout(wait, slots.acquire_owned())
        .await
        .map_err(|_| PoolError::Server("no handshake slot before the timeout".to_string()))?
        .map_err(|_| PoolError::Server("handshake limiter closed".to_string()))
}

/// Runs one synchronous handshake step under the shared handshake budget.
///
/// The step never awaits, so it occupies its runtime worker until it returns.
/// The CPU bound is therefore the runtime worker count: at most that many steps
/// run at once. The shared slot counts the steps in flight, and the network
/// waits are outside this function on purpose.
async fn crypto_step<T, E: std::fmt::Display>(
    gates: &Gates,
    step: impl FnOnce() -> std::result::Result<T, E>,
) -> Result<T> {
    let _slot = take_handshake_slot(Arc::clone(&gates.handshakes), HANDSHAKE_TIMEOUT).await?;
    step().map_err(|error| PoolError::Server(format!("post-quantum handshake failed: {error}")))
}

/// Bounds one network step of the handshake, naming the step in its error.
async fn within<T, E: std::fmt::Display>(
    wait: Duration,
    what: &str,
    io: impl Future<Output = std::result::Result<T, E>>,
) -> Result<T> {
    tokio::time::timeout(wait, io)
        .await
        .map_err(|_| PoolError::Server(format!("timed out {what}")))?
        .map_err(|error| PoolError::Server(format!("{what} failed: {error}")))
}

/// The responder half of the handshake, with every network wait bounded and
/// every CPU step under the shared budget.
///
/// The responder speaks first, so the order is: generate (CPU), send our key
/// (network), read the initiator's message (network), decapsulate (CPU). The
/// initiator's message is read whole, at its fixed length, so a peer that sends
/// part of it and stops is dropped at `io_wait`.
///
/// A silent connect costs one key generation. That is inherent to a
/// responder-first protocol: the pool cannot know the peer is silent until it
/// has sent its key.
async fn handshake_as_responder<S>(
    stream: &mut S,
    gates: &Gates,
    io_wait: Duration,
) -> Result<handshake::SessionKeys>
where
    S: futures::AsyncRead + futures::AsyncWrite + Unpin,
{
    let pending = crypto_step(gates, handshake::begin_respond).await?;
    let first = pending.first_message();
    within(
        io_wait,
        "sending the handshake key",
        stream.write_all(&first),
    )
    .await?;

    let mut inbound = vec![0u8; handshake::INITIATOR_MESSAGE_LEN];
    within(
        io_wait,
        "reading the initiator's handshake",
        stream.read_exact(&mut inbound),
    )
    .await?;
    crypto_step(gates, move || pending.finish(&inbound)).await
}

/// The whole responder handshake under one hard deadline.
///
/// Each network wait and each budget wait has its own bound, but their sum is
/// larger than any honest peer needs. This deadline is the cap: a handshake that
/// has not finished when it expires is abandoned, whichever step it was in.
async fn handshake_within_deadline<S>(
    stream: &mut S,
    gates: &Gates,
    overall: Duration,
    io_wait: Duration,
) -> Result<handshake::SessionKeys>
where
    S: futures::AsyncRead + futures::AsyncWrite + Unpin,
{
    match tokio::time::timeout(overall, handshake_as_responder(stream, gates, io_wait)).await {
        Ok(keys) => keys,
        Err(_) => Err(PoolError::Server(
            "post-quantum handshake exceeded its deadline".to_string(),
        )),
    }
}

/// Drives one miner connection from handshake to close.
async fn serve_connection(
    stream: TcpStream,
    peer: SocketAddr,
    state: Arc<PoolState>,
    validator: Validator,
    mut jobs: broadcast::Receiver<Arc<Job>>,
    gates: Gates,
    admitted: (PreAuthSlot, oneshot::Receiver<()>),
) -> Result<()> {
    stream.set_nodelay(true)?;
    let (preauth, mut evicted) = admitted;

    // The compat layer exists for exactly this line: the handshake is written
    // against futures' traits and a Tokio socket implements Tokio's.
    let mut compat = stream.compat();
    let handshake =
        handshake_within_deadline(&mut compat, &gates, HANDSHAKE_TIMEOUT, HANDSHAKE_IO_TIMEOUT);
    let keys = tokio::select! {
        biased;
        // A newer connection from this address took this one's place. The peer
        // is not lost: it reconnects, and this socket is simply dropped.
        _ = &mut evicted => return Ok(()),
        keys = handshake => keys?,
    };

    // Pre-auth entry released here; from now on the address's farm budget applies.
    drop(preauth);
    // A ghost connection, authenticated and then silent, holds this slot until
    // `IDLE_TIMEOUT` closes it. Keepalive via socket2 would reap it sooner, and
    // evicting the oldest authenticated entry would free the slot at once; both
    // are follow-ups.
    let key = address_key(peer.ip());
    let _authenticated = match IpSlot::claim(&gates.authenticated, key, gates.per_ip_cap) {
        Some(slot) => slot,
        None => {
            report_closed(&gates.log, peer, "authenticated cap reached");
            return Ok(());
        }
    };

    // `is_initiator: false` — the pool responds, so it opens with the key the
    // dialer seals with. Getting this backwards yields a stream that decrypts
    // to noise rather than one that fails to connect.
    let secure = PqStream::new(compat, keys, false);
    let (mut reader, writer) = secure.split();

    let (outbound, queued) = mpsc::channel::<Message>(OUTBOUND_CAPACITY);
    let mut write_task = tokio::spawn(write_frames(writer, queued, WRITE_TIMEOUT));

    // Job pushes travel by their own task, so the read loop never has to sit in
    // a `select!` beside a half-read frame.
    let channels: Arc<std::sync::Mutex<Vec<u32>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let push_channels = Arc::clone(&channels);
    let push_outbound = outbound.clone();
    let push_task = tokio::spawn(async move {
        loop {
            let job = match jobs.recv().await {
                Ok(job) => job,
                // A connection that fell behind on job pushes skips to the
                // newest rather than replaying stale work.
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            };

            let held: Vec<u32> = match push_channels.lock() {
                Ok(held) => held.clone(),
                Err(_) => break,
            };
            for channel_id in held {
                for message in job_messages(channel_id, &job) {
                    if push_outbound.send(message).await.is_err() {
                        return;
                    }
                }
            }
        }
    });

    let mut session = Session::new();
    let mut acks = AckBook::default();
    let mut invalid = InvalidShareBudget::default();
    // Set when the budget closes the connection: that close is already counted
    // on the shared log, so it is not also printed per connection.
    let mut budget_closed = false;

    let result = loop {
        // The timeout drops a half-read frame, which is safe only because the
        // connection ends on that path and nothing reads the stream again.
        let frame = match tokio::time::timeout(IDLE_TIMEOUT, read_frame(&mut reader)).await {
            Ok(Ok(Some(frame))) => frame,
            Ok(Ok(None)) => break Ok(()),
            Ok(Err(error)) => break Err(error),
            Err(_) => break Err(PoolError::Server("idle past the timeout".to_string())),
        };

        let message = match Message::from_frame(&frame) {
            Ok(message) => message,
            // A frame the pool cannot parse ends the connection. It is not an
            // error worth retrying: a client that sent one will send another.
            Err(error) => break Err(PoolError::Protocol(error)),
        };

        // Not `?`: an error here must still reach the cleanup below, or the
        // channel and its nonce range outlive the connection.
        let actions = match session.handle(message, &state) {
            Ok(actions) => actions,
            Err(error) => break Err(error),
        };
        if let Ok(mut held) = channels.lock() {
            *held = session.channels().to_vec();
        }

        let mut closing = None;
        for action in actions {
            match action {
                SessionAction::Reply(message) => {
                    if !enqueue(&outbound, message, SEND_TIMEOUT).await {
                        closing = Some("the connection stopped draining".to_string());
                    }
                }
                SessionAction::Close(reason) => closing = Some(reason),
                SessionAction::Validate(request) => {
                    let channel_id = request.channel_id;
                    let sequence_number = request.sequence_number;
                    let verdict = match validator.submit(request).await {
                        Ok(verdict) => verdict,
                        Err(error) => {
                            // Ends the connection through the cleanup path below,
                            // rather than returning past it.
                            closing = Some(error.to_string());
                            break;
                        }
                    };

                    // Every rejection here cost a full DAG verification, so a
                    // connection that keeps sending junk is closed, not served.
                    // Stale and duplicate verdicts were refused without verification, so
                    // they spend nothing and are not counted.
                    let counted = match verdict.outcome {
                        ShareOutcome::Accepted { .. } => Some(true),
                        ShareOutcome::Rejected(RejectReason::Stale | RejectReason::Duplicate) => {
                            None
                        }
                        ShareOutcome::Rejected(_) => Some(false),
                    };
                    if let Some(accepted) = counted
                        && invalid.record(accepted)
                    {
                        report_closed(&gates.log, peer, "too many invalid shares");
                        budget_closed = true;
                        closing = Some("too many invalid shares".to_string());
                    }

                    match verdict.outcome {
                        ShareOutcome::Accepted { weight, .. } => {
                            if let Some(ack) = acks.record(channel_id, sequence_number, weight)
                                && !enqueue(&outbound, ack, SEND_TIMEOUT).await
                            {
                                closing = Some("the connection stopped draining".to_string());
                            }
                        }
                        ShareOutcome::Rejected(reason) => {
                            // Flushed first: an acknowledgement covering shares
                            // *before* the rejected one must not arrive after
                            // it, or the miner cannot tell which failed.
                            if let Some(ack) = acks.flush(channel_id)
                                && !enqueue(&outbound, ack, SEND_TIMEOUT).await
                            {
                                closing = Some("the connection stopped draining".to_string());
                            }
                            let error = Message::SubmitSharesError(
                                maya_stratum_v2::messages::SubmitSharesError {
                                    channel_id,
                                    sequence_number,
                                    error_code: reason.code().to_string(),
                                },
                            );
                            if !enqueue(&outbound, error, SEND_TIMEOUT).await {
                                closing = Some("the connection stopped draining".to_string());
                            }
                        }
                    }

                    if let Some(target) = verdict.retune {
                        let retune = Message::SetTarget(maya_stratum_v2::messages::SetTarget {
                            channel_id,
                            maximum_target: target,
                        });
                        if !enqueue(&outbound, retune, SEND_TIMEOUT).await {
                            closing = Some("the connection stopped draining".to_string());
                        }
                    }
                }
            }
        }

        if let Some(reason) = closing {
            for channel_id in session.channels() {
                let _ = outbound.try_send(close_message(*channel_id, "pool-closing"));
            }
            break if budget_closed {
                Ok(())
            } else {
                Err(PoolError::Server(reason))
            };
        }
    };

    // Whatever ended the connection, its channels have to go back: a leaked
    // channel holds a nonce range forever and leaves its rig showing as
    // connected on the dashboard.
    // Pending acknowledgements are still owed to the miner: the shares they
    // cover were accepted and credited before the connection ended.
    for ack in acks.drain() {
        let _ = outbound.try_send(ack);
    }
    if let Err(error) = session.disconnect(&state) {
        eprintln!("pool: releasing a connection's channels failed: {error}");
    }
    drop(outbound);
    push_task.abort();
    // Bounded: a peer that stopped reading must not hold the permit. The write
    // task is aborted if it has not drained by the deadline.
    if tokio::time::timeout(WRITE_TIMEOUT, &mut write_task)
        .await
        .is_err()
    {
        write_task.abort();
    }
    result
}

/// Writes queued messages to the socket until the queue closes or a write stalls.
async fn write_frames<W: futures::io::AsyncWrite + Unpin>(
    mut writer: W,
    mut queued: mpsc::Receiver<Message>,
    write_timeout: Duration,
) {
    while let Some(message) = queued.recv().await {
        let Ok(frame) = message.to_frame() else {
            // A message the pool itself could not encode is a pool bug, and
            // dropping the connection over it would hide that.
            eprintln!("pool: refusing to send an unencodable message");
            continue;
        };
        let bytes = frame.encode();
        let sent = tokio::time::timeout(write_timeout, async {
            writer.write_all(&bytes).await?;
            writer.flush().await
        })
        .await;
        if !matches!(sent, Ok(Ok(()))) {
            break;
        }
    }
}

/// Reads one SV2 frame, or `None` at a clean end of stream.
async fn read_frame<R: futures::AsyncRead + Unpin>(reader: &mut R) -> Result<Option<Frame>> {
    read_frame_within(reader, FRAME_TIMEOUT).await
}

/// [`read_frame`] with the payload deadline as a parameter, so a stall can be
/// tested without waiting out [`FRAME_TIMEOUT`].
async fn read_frame_within<R: futures::AsyncRead + Unpin>(
    reader: &mut R,
    payload_timeout: Duration,
) -> Result<Option<Frame>> {
    let mut header_bytes = [0u8; maya_stratum_v2::frame::FRAME_HEADER_LEN];
    match reader.read_exact(&mut header_bytes).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => {
            return Err(PoolError::Server(format!(
                "reading a frame header: {error}"
            )));
        }
    }

    let header = FrameHeader::decode(&header_bytes)?;
    // Checked before a byte is reserved. At 50,000 connections a length field is
    // an allocation primitive, which is why `MAX_PAYLOAD_LEN` is set from the
    // transport's ceiling rather than from the U24 field's 16 MiB.
    if header.msg_length as usize > MAX_PAYLOAD_LEN {
        return Err(PoolError::Protocol(
            maya_stratum_v2::Sv2Error::PayloadTooLarge {
                len: header.msg_length as usize,
                max: MAX_PAYLOAD_LEN,
            },
        ));
    }

    // Grows as bytes arrive instead of reserving the declared length up front: a
    // header alone should not pin up to 64 KiB per connection.
    let len = header.msg_length as usize;
    let mut payload = Vec::new();
    let mut limited = AsyncReadExt::take(&mut *reader, len as u64);
    let read = tokio::time::timeout(payload_timeout, limited.read_to_end(&mut payload))
        .await
        .map_err(|_| PoolError::Server("a frame's payload stalled past its deadline".to_string()))?
        .map_err(|error| PoolError::Server(format!("reading a frame payload: {error}")))?;
    if read != len {
        return Err(PoolError::Server(format!(
            "connection closed inside a frame: {read} of {len} payload bytes"
        )));
    }

    let mut whole = header_bytes.to_vec();
    whole.extend_from_slice(&payload);
    Ok(Some(Frame::decode(&whole)?))
}

/// Pending acknowledgements, per channel.
///
/// SV2 acknowledges shares in bulk; this is the accumulator that makes that
/// true rather than aspirational.
#[derive(Debug, Default)]
struct AckBook {
    /// One entry per channel with unacknowledged shares.
    pending: std::collections::HashMap<u32, PendingAck>,
}

/// Shares accepted on one channel since its last acknowledgement.
#[derive(Debug, Default)]
struct PendingAck {
    /// Highest sequence number covered.
    last_sequence: u32,
    /// Shares accepted.
    count: u32,
    /// Weight credited.
    weight: u64,
}

impl AckBook {
    /// Records an accepted share, returning an acknowledgement when one is due.
    fn record(&mut self, channel_id: u32, sequence: u32, weight: u64) -> Option<Message> {
        let entry = self.pending.entry(channel_id).or_default();
        entry.last_sequence = sequence;
        entry.count += 1;
        entry.weight = entry.weight.saturating_add(weight);

        if entry.count >= ACK_BATCH {
            return self.flush(channel_id);
        }
        None
    }

    /// Emits every pending acknowledgement, for the paths that end a connection.
    fn drain(&mut self) -> Vec<Message> {
        let channels: Vec<u32> = self.pending.keys().copied().collect();
        channels
            .into_iter()
            .filter_map(|channel_id| self.flush(channel_id))
            .collect()
    }

    /// Emits an acknowledgement for whatever is pending on a channel.
    fn flush(&mut self, channel_id: u32) -> Option<Message> {
        let entry = self.pending.remove(&channel_id)?;
        if entry.count == 0 {
            return None;
        }

        Some(Message::SubmitSharesSuccess(
            maya_stratum_v2::messages::SubmitSharesSuccess {
                channel_id,
                last_sequence_number: entry.last_sequence,
                new_submits_accepted_count: entry.count,
                new_shares_sum: entry.weight,
            },
        ))
    }
}

/// Polls the node for work and pushes it to every channel.
///
/// # Errors
///
/// Never returns while the process is alive; failures are logged and retried,
/// because an unreachable node is a condition to survive rather than to exit
/// on.
pub async fn job_loop(
    state: Arc<PoolState>,
    node: Arc<NodeClient>,
    jobs: broadcast::Sender<Arc<Job>>,
) {
    let mut ticker = tokio::time::interval(JOB_POLL_INTERVAL);

    loop {
        ticker.tick().await;

        let candidate = match node.mining_candidate().await {
            Ok(candidate) => candidate,
            Err(error) => {
                eprintln!("pool: no work from the node: {error}");
                continue;
            }
        };

        let started = std::time::Instant::now();
        let pushed = {
            let mut registry = match state.jobs_mut() {
                Ok(registry) => registry,
                Err(error) => {
                    eprintln!("pool: {error}");
                    continue;
                }
            };

            // `None` means the node handed back the same work with a newer
            // timestamp, which it does on every call. The registry is what
            // decides that, so this loop cannot get it wrong by omission —
            // see `JobRegistry::push`.
            match registry.push(&candidate) {
                Ok(Some(id)) => registry.get(id).cloned(),
                Ok(None) => None,
                Err(error) => {
                    eprintln!("pool: {error}");
                    continue;
                }
            }
        };

        let Some(job) = pushed else {
            continue;
        };

        if let Ok(mut channels) = state.channels() {
            for channel in channels.iter_mut() {
                channel.push_job(job.id);
            }
        }

        let job = Arc::new(job);
        let _ = jobs.send(Arc::clone(&job));
        state
            .metrics
            .observe_job_push(started.elapsed().as_secs_f64());
        state.publish(PoolEvent::Job {
            id: job.id,
            height: job.height,
        });
    }
}

/// Submits solved blocks and credits their PPLNS windows.
pub async fn block_loop(
    state: Arc<PoolState>,
    node: Arc<NodeClient>,
    engine: Arc<PayoutEngine>,
    mut found: mpsc::Receiver<FoundBlockShare>,
) {
    while let Some(share) = found.recv().await {
        let job = {
            let Ok(jobs) = state.jobs() else { continue };
            jobs.get(share.job_id).cloned()
        };
        let Some(job) = job else {
            eprintln!("pool: a solved block's job was retired before it could be submitted");
            continue;
        };

        let block = share::block_for(&job, share.nonce, share.ntime);
        let id = hex::encode(block.header.id());

        if let Err(error) = node.submit_block(&block).await {
            // The share is already credited, which is right: the miner did the
            // work. What is lost is the block, and that must be loud.
            eprintln!("pool: the chain refused a block the pool verified: {error}");
            continue;
        }

        state.metrics.record_block_found();
        state.publish(PoolEvent::Block {
            id: id.clone(),
            height: job.height,
            finder: hex::encode(share.key.miner),
        });

        if let Err(error) = engine.credit_block(
            &id,
            job.height,
            share.sequence,
            share.key.miner,
            &job.network_target,
        ) {
            // The block is on the chain and its credits are not in the ledger.
            // Nothing here can fix that automatically, so it is reported in the
            // strongest terms the daemon has.
            eprintln!("pool: BLOCK {id} WAS SUBMITTED BUT NOT CREDITED: {error}");
        }
    }
}

/// Runs the payout engine on a timer.
pub async fn settle_loop(state: Arc<PoolState>, engine: Arc<PayoutEngine>) {
    let mut ticker = tokio::time::interval(SETTLE_INTERVAL);

    loop {
        ticker.tick().await;

        match engine.settle().await {
            Ok(report) => {
                state.metrics.record_block_orphaned(report.orphaned as u64);
                state.metrics.record_payouts(
                    report.broadcast as u64,
                    report.confirmed as u64,
                    // shortcut: Prometheus counters are u64, so a value above it saturates; the ledger keeps the exact figure.
                    crate::node::saturating_u64(report.value_broadcast),
                );
            }
            Err(PoolError::Treasury(reason)) => {
                // Never retried on its own: every case is a policy or custody
                // condition that no retry clears.
                state.metrics.record_payout_refused();
                eprintln!("pool: payouts are stopped — {reason}");
            }
            Err(error) => eprintln!("pool: settlement pass failed: {error}"),
        }
    }
}

/// Resamples gauges, eases idle channels, and prunes.
pub async fn maintenance_loop(state: Arc<PoolState>, node: Arc<NodeClient>, treasury: [u8; 32]) {
    let mut ticker = tokio::time::interval(MAINTENANCE_INTERVAL);

    loop {
        ticker.tick().await;
        let now = now_millis();

        if let Err(error) = state.sample_gauges(now) {
            eprintln!("pool: {error}");
        }

        // The treasury balance is the gauge to page on: this chain mints no
        // block reward, so an empty account means every payout stops.
        match node.account_info(&treasury).await {
            Ok(info) => state
                .metrics
                .set_treasury_balance(crate::node::saturating_u64(info.balance)),
            Err(error) => eprintln!("pool: could not read the treasury balance: {error}"),
        }

        if let Ok(mut channels) = state.channels() {
            for channel in channels.iter_mut() {
                channel.vardiff.on_idle_check(now);
            }
        }

        if let Ok(mut telemetry) = state.telemetry() {
            telemetry.evict_stale(now);
        }

        prune(&state);
    }
}

/// Drops share records far behind the current PPLNS window.
fn prune(state: &PoolState) {
    let Ok(jobs) = state.jobs() else { return };
    let Some(job) = jobs.current() else { return };
    let network_target = job.network_target;
    drop(jobs);

    let Ok(window) = crate::pplns::window_size(&network_target, state.config.pplns_factor) else {
        return;
    };
    let Ok(Some(tip)) = state.ledger.tip_sequence() else {
        return;
    };

    // Walk back four windows' worth and prune below that. Deriving the cut from
    // the window itself rather than from a share count is what keeps the rule
    // correct when vardiff has moved: the ledger keeps a fixed amount of
    // *work*, not a fixed number of records.
    let Some(deep) = window.mul_div_u64(PRUNE_WINDOW_MULTIPLE, 1) else {
        // Four windows overflowed 256 bits, which means the window itself is
        // near the top of the range. Nothing to prune, and nothing to guess at.
        return;
    };
    let Ok(slice) = state.ledger.window(tip, deep) else {
        return;
    };
    if !slice.full {
        // The ledger does not yet hold four windows, so there is nothing behind
        // them to drop.
        return;
    }

    let Some(oldest) = slice.shares.last().map(|share| share.sequence) else {
        return;
    };
    if let Err(error) = state.ledger.prune_shares_below(oldest) {
        eprintln!("pool: pruning failed: {error}");
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn acknowledgements_are_batched_rather_than_per_share() {
        // A reply per submission doubles the pool's write syscalls to carry no
        // information the miner did not already have.
        let mut acks = AckBook::default();

        for sequence in 0..ACK_BATCH - 1 {
            assert!(acks.record(1, sequence, 1_024).is_none());
        }

        match acks.record(1, ACK_BATCH - 1, 1_024) {
            Some(Message::SubmitSharesSuccess(success)) => {
                assert_eq!(success.new_submits_accepted_count, ACK_BATCH);
                assert_eq!(success.last_sequence_number, ACK_BATCH - 1);
                assert_eq!(success.new_shares_sum, u64::from(ACK_BATCH) * 1_024);
            }
            other => panic!("expected a batched acknowledgement, got {other:?}"),
        }
    }

    #[test]
    fn a_flush_covers_only_what_is_pending() {
        let mut acks = AckBook::default();
        acks.record(1, 0, 10);
        acks.record(1, 1, 10);

        match acks.flush(1) {
            Some(Message::SubmitSharesSuccess(success)) => {
                assert_eq!(success.new_submits_accepted_count, 2);
                assert_eq!(success.new_shares_sum, 20);
            }
            other => panic!("expected an acknowledgement, got {other:?}"),
        }

        // And nothing is left to acknowledge twice.
        assert!(acks.flush(1).is_none());
    }

    #[test]
    fn channels_are_acknowledged_independently() {
        // One rig's share rate must not decide when another's shares are
        // acknowledged.
        let mut acks = AckBook::default();
        for sequence in 0..ACK_BATCH {
            acks.record(1, sequence, 1);
        }
        acks.record(2, 0, 1);

        match acks.flush(2) {
            Some(Message::SubmitSharesSuccess(success)) => {
                assert_eq!(success.channel_id, 2);
                assert_eq!(success.new_submits_accepted_count, 1);
            }
            other => panic!("expected channel 2's own acknowledgement, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_truncated_frame_header_is_a_clean_end_of_stream() {
        // A miner that hangs up between messages is ordinary, and must not be
        // logged as a protocol violation.
        let empty: &[u8] = &[];
        let mut reader = futures::io::Cursor::new(empty);
        assert!(read_frame(&mut reader).await.unwrap().is_none());
    }

    /// Yields its prefix, then never another byte: a peer that sent a header and stopped.
    struct StalledAfter {
        prefix: futures::io::Cursor<Vec<u8>>,
    }

    impl futures::AsyncRead for StalledAfter {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &mut [u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            let this = self.get_mut();
            match futures::AsyncRead::poll_read(std::pin::Pin::new(&mut this.prefix), cx, buf) {
                std::task::Poll::Ready(Ok(0)) => std::task::Poll::Pending,
                other => other,
            }
        }
    }

    #[tokio::test]
    async fn a_payload_that_stalls_after_its_header_is_abandoned_at_the_deadline() {
        // The header promises 100 bytes and then nothing: the connection must not
        // wait out the idle timeout, nor reserve the 100 bytes it never received.
        let mut header = [0u8; maya_stratum_v2::frame::FRAME_HEADER_LEN];
        header[0..2].copy_from_slice(&maya_stratum_v2::MAYA_EXTENSION_TYPE.to_le_bytes());
        header[2] = 0x1a;
        header[3..6].copy_from_slice(&[100, 0, 0]);

        let mut reader = StalledAfter {
            prefix: futures::io::Cursor::new(header.to_vec()),
        };
        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            read_frame_within(&mut reader, Duration::from_millis(20)),
        )
        .await;
        assert!(
            matches!(outcome, Ok(Err(_))),
            "a stalled payload was not abandoned at its deadline"
        );
    }

    #[tokio::test]
    async fn a_frame_declaring_more_than_the_transport_can_carry_is_refused() {
        // Refused before a byte is reserved: at 50,000 connections a length
        // field is an allocation primitive.
        let mut header = [0u8; maya_stratum_v2::frame::FRAME_HEADER_LEN];
        header[0..2].copy_from_slice(&maya_stratum_v2::MAYA_EXTENSION_TYPE.to_le_bytes());
        header[2] = 0x1a;
        header[3..6].copy_from_slice(&[0xFF, 0xFF, 0xFF]);

        let mut reader = futures::io::Cursor::new(header.to_vec());
        assert!(read_frame(&mut reader).await.is_err());
    }

    #[tokio::test]
    async fn a_well_formed_frame_round_trips_off_the_wire() {
        let message =
            Message::SubmitSharesStandard(maya_stratum_v2::messages::SubmitSharesStandard {
                channel_id: 1,
                sequence_number: 2,
                job_id: 3,
                nonce: 4,
                ntime: 5,
            });
        let bytes = message.to_frame().unwrap().encode();

        let mut reader = futures::io::Cursor::new(bytes);
        let frame = read_frame(&mut reader).await.unwrap().unwrap();
        assert_eq!(Message::from_frame(&frame).unwrap(), message);
    }

    /// A writer whose socket never drains, the way a peer that stopped reading looks.
    struct Stalled;

    impl futures::io::AsyncWrite for Stalled {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            _: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::task::Poll::Pending
        }

        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Pending
        }

        fn poll_close(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Pending
        }
    }

    #[tokio::test]
    async fn a_writer_that_stops_draining_is_abandoned_after_the_write_timeout() {
        // Before the fix the write task never returned on a peer that stopped
        // reading, and the connection's admission permit went with it.
        let (queue, queued) = mpsc::channel(4);
        queue
            .send(Message::SubmitSharesStandard(
                maya_stratum_v2::messages::SubmitSharesStandard {
                    channel_id: 1,
                    sequence_number: 1,
                    job_id: 1,
                    nonce: 1,
                    ntime: 1,
                },
            ))
            .await
            .unwrap();

        let finished = tokio::time::timeout(
            Duration::from_secs(5),
            write_frames(Stalled, queued, Duration::from_millis(20)),
        )
        .await;
        assert!(
            finished.is_ok(),
            "write task still stuck on a peer that never reads"
        );
        drop(queue);
    }

    #[tokio::test]
    async fn a_full_queue_with_a_draining_reader_is_not_closed() {
        // A job-push burst fills the queue briefly; the reader catches up and the
        // reply must still go out rather than close a healthy miner.
        let (outbound, mut queued) = mpsc::channel::<Message>(1);
        let ping = || {
            Message::SubmitSharesError(maya_stratum_v2::messages::SubmitSharesError {
                channel_id: 1,
                sequence_number: 1,
                error_code: String::new(),
            })
        };
        outbound.try_send(ping()).unwrap();
        let reader = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            while queued.recv().await.is_some() {}
        });

        assert!(enqueue(&outbound, ping(), Duration::from_secs(2)).await);
        drop(outbound);
        reader.await.unwrap();
    }

    #[tokio::test]
    async fn a_full_queue_that_never_drains_is_closed_at_the_timeout() {
        let (outbound, _queued) = mpsc::channel::<Message>(1);
        let ping = || {
            Message::SubmitSharesError(maya_stratum_v2::messages::SubmitSharesError {
                channel_id: 1,
                sequence_number: 1,
                error_code: String::new(),
            })
        };
        outbound.try_send(ping()).unwrap();
        assert!(!enqueue(&outbound, ping(), Duration::from_millis(20)).await);
    }

    #[test]
    fn draining_emits_every_pending_acknowledgement_once() {
        // Shares accepted just before a connection ends are still owed an ack.
        let mut acks = AckBook::default();
        acks.record(1, 0, 10);
        acks.record(2, 0, 10);
        assert_eq!(acks.drain().len(), 2);
        assert!(acks.drain().is_empty());
    }

    #[test]
    fn one_address_cannot_hold_more_than_its_cap_and_dropping_frees_a_slot() {
        // NAT farms share an address, so the cap has to be real but generous.
        let counts: PerIpCounts = Arc::new(Mutex::new(HashMap::new()));
        let ip: IpAddr = "203.0.113.7".parse().unwrap();

        let cap = crate::config::DEFAULT_MAX_CONNECTIONS_PER_IP;
        let held: Vec<IpSlot> = (0..cap)
            .map(|_| IpSlot::claim(&counts, ip, cap).unwrap())
            .collect();
        assert!(IpSlot::claim(&counts, ip, cap).is_none());

        // A different address is unaffected by this one's cap.
        let other: IpAddr = "203.0.113.8".parse().unwrap();
        assert!(IpSlot::claim(&counts, other, cap).is_some());

        drop(held);
        assert!(IpSlot::claim(&counts, ip, cap).is_some());
    }

    #[test]
    fn an_address_with_no_open_connections_leaves_no_entry_behind() {
        // The map would otherwise grow by one entry per stranger that ever connected.
        let counts: PerIpCounts = Arc::new(Mutex::new(HashMap::new()));
        let ip: IpAddr = "198.51.100.1".parse().unwrap();

        let cap = crate::config::DEFAULT_MAX_CONNECTIONS_PER_IP;
        drop(IpSlot::claim(&counts, ip, cap).unwrap());
        assert!(counts.lock().unwrap().is_empty());
    }

    #[test]
    fn unauthenticated_connections_do_not_use_the_authenticated_budget() {
        // A neighbour holding every pre-auth entry on a shared address must not
        // stop the farm's authenticated connections.
        let gates = Gates::new(crate::config::DEFAULT_MAX_CONNECTIONS_PER_IP);
        let ip: IpAddr = "192.0.2.10".parse().unwrap();

        let mut held = Vec::new();
        for _ in 0..PREAUTH_PER_IP {
            let (id, _signal, _evicted) = gates.preauth.lock().unwrap().admit(ip, PREAUTH_PER_IP);
            held.push(PreAuthSlot {
                registry: Arc::clone(&gates.preauth),
                ip,
                id,
            });
        }

        assert!(IpSlot::claim(&gates.authenticated, ip, gates.per_ip_cap).is_some());
        drop(held);
    }

    #[test]
    fn a_newcomer_at_the_pre_auth_cap_evicts_the_oldest_entry() {
        let mut registry = PreAuthRegistry::default();
        let ip: IpAddr = "192.0.2.11".parse().unwrap();

        let mut signals = Vec::new();
        for _ in 0..PREAUTH_PER_IP {
            let (_, signal, evicted_any) = registry.admit(ip, PREAUTH_PER_IP);
            assert!(!evicted_any);
            signals.push(signal);
        }

        let (_, _, evicted_any) = registry.admit(ip, PREAUTH_PER_IP);
        assert!(evicted_any, "the newcomer displaces an older entry");
        assert!(signals[0].try_recv().is_ok(), "the oldest was signalled");
        assert!(
            signals[1].try_recv().is_err(),
            "the next oldest is still waiting"
        );
    }

    #[test]
    fn a_pre_auth_entry_is_forgotten_when_its_connection_ends() {
        let registry: PreAuth = Arc::new(Mutex::new(PreAuthRegistry::default()));
        let ip: IpAddr = "192.0.2.12".parse().unwrap();

        let (id, _signal, _evicted) = registry.lock().unwrap().admit(ip, PREAUTH_PER_IP);
        drop(PreAuthSlot {
            registry: Arc::clone(&registry),
            ip,
            id,
        });
        assert!(registry.lock().unwrap().by_ip.is_empty());
    }

    #[test]
    fn ipv6_budgets_are_keyed_on_the_64_bit_prefix() {
        // Two addresses in one /64 share a budget; a different /64 does not.
        // IPv4 keeps its full address.
        let first: IpAddr = "2001:db8:1:2:3:4:5:6".parse().unwrap();
        let same_prefix: IpAddr = "2001:db8:1:2:ffff:ffff:ffff:ffff".parse().unwrap();
        let other_prefix: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        let v4: IpAddr = "192.0.2.77".parse().unwrap();

        assert_eq!(address_key(first), address_key(same_prefix));
        assert_ne!(address_key(first), address_key(other_prefix));
        assert_eq!(address_key(v4), v4);
    }

    #[test]
    fn connects_burst_then_are_refused_until_the_bucket_refills() {
        // A burst is allowed; past it the address waits for tokens at the sustained rate.
        let mut buckets = HashMap::new();
        let ip: IpAddr = "198.51.100.9".parse().unwrap();
        let start = Instant::now();

        for _ in 0..CONNECT_BURST {
            assert!(take_connect_token(&mut buckets, ip, start));
        }
        assert!(!take_connect_token(&mut buckets, ip, start));

        // At 4 a second, one token returns every 250 ms.
        let quarter = start + Duration::from_millis(250);
        assert!(take_connect_token(&mut buckets, ip, quarter));
        assert!(!take_connect_token(&mut buckets, ip, quarter));
    }

    #[tokio::test]
    async fn a_handshake_that_stalls_between_steps_is_cut_at_the_overall_deadline() {
        // Each wait is bounded on its own, but the handshake as a whole must end
        // even when no single wait does.
        use tokio::io::AsyncWriteExt;

        let (server, mut client) = socket_pair().await;
        client.write_all(&[1]).await.unwrap();
        let gates = Gates::new(crate::config::DEFAULT_MAX_CONNECTIONS_PER_IP);

        let mut compat = server.compat();
        let outcome = handshake_within_deadline(
            &mut compat,
            &gates,
            Duration::from_millis(50),
            Duration::from_secs(60),
        )
        .await;
        match outcome {
            Err(PoolError::Server(message)) => {
                assert!(message.contains("deadline"), "got: {message}");
            }
            other => panic!("expected the overall deadline, got {:?}", other.map(|_| ())),
        }
    }

    /// A connected pair of loopback sockets, the server side first.
    async fn socket_pair() -> (TcpStream, TcpStream) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let client = TcpStream::connect(address).await.unwrap();
        let (server, _) = listener.accept().await.unwrap();
        (server, client)
    }

    #[tokio::test]
    async fn one_byte_then_stall_holds_no_global_handshake_slot() {
        // The responder waits on the initiator's whole message. That wait is a
        // network wait, and during it no shared handshake slot may be held.
        use tokio::io::AsyncWriteExt;

        let (server, mut client) = socket_pair().await;
        client.write_all(&[1]).await.unwrap();
        let gates = Gates::new(crate::config::DEFAULT_MAX_CONNECTIONS_PER_IP);

        let mut compat = server.compat();
        let waiting = {
            let gates = gates.clone();
            tokio::spawn(async move {
                handshake_as_responder(&mut compat, &gates, Duration::from_millis(300)).await
            })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            gates.handshakes.available_permits(),
            MAX_CONCURRENT_HANDSHAKES,
            "the peer is mid-message, so no slot may be held"
        );
        assert!(
            waiting.await.unwrap().is_err(),
            "a stalled message is dropped at its deadline"
        );
    }

    #[tokio::test]
    async fn a_partial_first_message_then_a_stall_is_dropped_at_the_deadline() {
        use tokio::io::AsyncWriteExt;

        let (server, mut client) = socket_pair().await;
        client.write_all(&[1; 10]).await.unwrap();
        let gates = Gates::new(crate::config::DEFAULT_MAX_CONNECTIONS_PER_IP);

        let mut compat = server.compat();
        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            handshake_as_responder(&mut compat, &gates, Duration::from_millis(50)),
        )
        .await;
        assert!(
            matches!(outcome, Ok(Err(_))),
            "a partial message was not dropped at its deadline"
        );
    }

    #[tokio::test]
    async fn a_connect_flood_cannot_hold_every_handshake_slot() {
        // Once the slots are held, the next connection is refused at its wait
        // rather than queued behind the flood, and a released slot is reusable.
        let wait = Duration::from_millis(20);
        let slots = Arc::new(Semaphore::new(2));
        let first = take_handshake_slot(Arc::clone(&slots), wait).await.unwrap();
        let second = take_handshake_slot(Arc::clone(&slots), wait).await.unwrap();
        assert!(take_handshake_slot(Arc::clone(&slots), wait).await.is_err());

        drop(first);
        assert!(take_handshake_slot(Arc::clone(&slots), wait).await.is_ok());
        drop(second);
    }

    #[test]
    fn refusals_are_reported_in_batches_not_per_connection() {
        let start = Instant::now();
        let mut refusals = ThrottledLog::default();

        assert_eq!(
            refusals.record(start),
            Some(1),
            "the first refusal is reported"
        );
        for offset in 1..=5 {
            assert_eq!(refusals.record(start + Duration::from_millis(offset)), None);
        }
        assert_eq!(
            refusals.record(start + REFUSAL_LOG_INTERVAL),
            Some(6),
            "the next report carries everything refused since the last"
        );
    }
}
