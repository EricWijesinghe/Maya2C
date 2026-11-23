//! Admission to the RPC's scarce resources: connection slots, blocking
//! threads, and the one archive-fetch slot.
//!
//! Each refusal here is transient: the client is told to come back, and the
//! node's own bootstrap client retries it (`rpc::bootstrap::is_transient`).
//! None of this is consensus code; `Instant` and `HashMap` are fine here.

use std::collections::{BTreeMap, HashMap};
use std::net::IpAddr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use jsonrpsee::core::RegisterMethodError;
use jsonrpsee::core::server::{IntoResponse, MethodCallback, ResponsePayload};
use jsonrpsee::server::RpcModule;
use jsonrpsee::types::{ErrorObjectOwned, Params};

use crate::rpc::limit::bucket_key;
use crate::rpc::server::RpcContext;

/// JSON-RPC error code for a request the node refused.
const REJECTED: i32 = -32_000;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---------------------------------------------------------------------------
// Blocking handlers
// ---------------------------------------------------------------------------

/// Blocking RPC handlers running at once, across all clients. They share
/// tokio's blocking pool (512 threads) with block relay, BFT catch-up, the
/// engine-window and rejoin probes, pruning and DAG prepare, and a handler is
/// not cancelled when its client hangs up, so without a bound a
/// send-and-close loop at the per-IP request rate filled the pool and node
/// work queued behind RPC reads. Not measured: far below the pool, above the
/// cores of a node that serves reads.
pub const BLOCKING_SLOTS: usize = 32;

/// The refusal a client gets while every blocking slot is taken.
pub const RPC_BUSY: &str = "the node is busy serving other reads; retry later";

/// Registering a handler that runs on a blocking thread, bounded by
/// [`RpcContext::blocking`].
pub trait RegisterBounded {
    /// As `register_blocking_method`, except that a call finding all
    /// [`BLOCKING_SLOTS`] taken is refused with [`RPC_BUSY`] before it takes
    /// a pool thread.
    ///
    /// # Errors
    ///
    /// Returns an error if `name` is already registered.
    fn register_bounded<R, F>(
        &mut self,
        name: &'static str,
        callback: F,
    ) -> Result<&mut MethodCallback, RegisterMethodError>
    where
        R: IntoResponse + Send + 'static,
        F: Fn(Params<'static>, Arc<RpcContext>, hyper::http::Extensions) -> R
            + Clone
            + Send
            + Sync
            + 'static;
}

impl RegisterBounded for RpcModule<RpcContext> {
    fn register_bounded<R, F>(
        &mut self,
        name: &'static str,
        callback: F,
    ) -> Result<&mut MethodCallback, RegisterMethodError>
    where
        R: IntoResponse + Send + 'static,
        F: Fn(Params<'static>, Arc<RpcContext>, hyper::http::Extensions) -> R
            + Clone
            + Send
            + Sync
            + 'static,
    {
        self.register_async_method(name, move |params, ctx, extensions| {
            let callback = callback.clone();
            async move {
                let Ok(slot) = Arc::clone(&ctx.blocking).try_acquire_owned() else {
                    return ResponsePayload::error(ErrorObjectOwned::owned(
                        REJECTED, RPC_BUSY, None::<()>,
                    ));
                };
                // The slot moves into the task: a client that hangs up drops
                // this future, not the work, so the slot is held until the
                // work ends.
                tokio::task::spawn_blocking(move || {
                    let _slot = slot;
                    callback(params, ctx, extensions)
                })
                .await
                .map_or_else(
                    |_| ResponsePayload::error(crate::rpc::server::internal("handler panicked")),
                    IntoResponse::into_response,
                )
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Connections
// ---------------------------------------------------------------------------

/// Connections one socket peer may hold at once (an IPv6 peer by its /64).
/// Loopback and trusted proxies are exempt: every client behind them shares
/// their address.
pub const MAX_CONNECTIONS_PER_PEER: usize = 32;

/// Connections one network may hold at once: an IPv6 /48 (a common single
/// allocation, 65,536 /64s) or an IPv4 /24. Without it one allocation filled
/// every slot from per-peer shares alone.
pub const MAX_CONNECTIONS_PER_NETWORK: usize = 128;

/// The network an address is counted under for [`MAX_CONNECTIONS_PER_NETWORK`].
fn network_key(address: IpAddr) -> IpAddr {
    match bucket_key(address) {
        IpAddr::V4(v4) => {
            let [a, b, c, _] = v4.octets();
            IpAddr::V4(std::net::Ipv4Addr::new(a, b, c, 0))
        }
        IpAddr::V6(v6) => {
            let mut segments = v6.segments();
            segments[3..].fill(0);
            IpAddr::V6(std::net::Ipv6Addr::from(segments))
        }
    }
}

/// The open connections, and who holds them.
///
/// When every slot is taken, a newcomer is still served and the oldest
/// connection of the network holding the most is shed. Refusing the newcomer
/// instead let whoever filled the table first keep it: sockets that send a
/// small request every few seconds and reconnect at once never let an honest
/// client in.
#[derive(Debug)]
pub(crate) struct Connections {
    limit: usize,
    open: Mutex<Open>,
}

#[derive(Debug, Default)]
struct Open {
    next: u64,
    /// Oldest first.
    live: BTreeMap<u64, Live>,
    /// Entries of `live` told to close and not yet gone.
    shedding: usize,
    per_peer: HashMap<IpAddr, usize>,
    per_network: HashMap<IpAddr, usize>,
}

#[derive(Debug)]
struct Live {
    /// `None` for an exempt peer, which no per-peer cap counts.
    peer: Option<IpAddr>,
    network: IpAddr,
    shed: Arc<tokio::sync::Notify>,
    shedding: bool,
}

/// One connection's place in [`Connections`]; released on drop.
#[derive(Debug)]
pub(crate) struct ConnectionSlot {
    table: Arc<Connections>,
    id: u64,
    shed: Arc<tokio::sync::Notify>,
}

impl ConnectionSlot {
    /// Resolves when this connection is shed to make room for a newcomer.
    pub(crate) async fn shed(&self) {
        self.shed.notified().await;
    }
}

fn decrement(counts: &mut HashMap<IpAddr, usize>, key: IpAddr) {
    if let Some(count) = counts.get_mut(&key) {
        *count -= 1;
        if *count == 0 {
            counts.remove(&key);
        }
    }
}

impl Open {
    fn count(counts: &HashMap<IpAddr, usize>, key: IpAddr) -> usize {
        counts.get(&key).copied().unwrap_or(0)
    }

    /// Tells the oldest connection of the network holding the most to close,
    /// preferring `newcomer`'s own network on a tie, and stops counting it
    /// against any cap.
    fn shed_one(&mut self, newcomer: IpAddr) {
        let mine = Self::count(&self.per_network, newcomer);
        let heaviest = self
            .per_network
            .iter()
            .max_by_key(|(key, count)| (**count, **key))
            .map(|(key, count)| (*key, *count));
        let victim = match heaviest {
            Some((key, count)) if count > mine => key,
            _ => newcomer,
        };
        let Some(live) = self
            .live
            .values_mut()
            .find(|live| !live.shedding && live.network == victim)
        else {
            return;
        };
        live.shedding = true;
        live.shed.notify_one();
        let (peer, network) = (live.peer, live.network);
        self.shedding += 1;
        if let Some(peer) = peer {
            decrement(&mut self.per_peer, peer);
        }
        decrement(&mut self.per_network, network);
    }
}

impl Connections {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            limit,
            open: Mutex::default(),
        }
    }

    /// A slot for a connection from `address`, or `None` if its peer or
    /// network already holds its share, or so many shed connections are still
    /// closing that the socket count would pass an eighth over the limit.
    /// `exempt` (loopback, a trusted proxy) skips the shares, not the limit.
    pub(crate) fn claim(
        table: &Arc<Self>,
        address: IpAddr,
        exempt: bool,
    ) -> Option<ConnectionSlot> {
        let peer = bucket_key(address);
        let network = if exempt { peer } else { network_key(address) };
        let mut open = lock(&table.open);
        if !exempt
            && (Open::count(&open.per_peer, peer) >= MAX_CONNECTIONS_PER_PEER
                || Open::count(&open.per_network, network) >= MAX_CONNECTIONS_PER_NETWORK)
        {
            return None;
        }
        if open.live.len() - open.shedding >= table.limit {
            if open.live.len() >= table.limit + table.limit / 8 {
                return None;
            }
            open.shed_one(network);
        }
        let id = open.next;
        open.next += 1;
        let shed = Arc::new(tokio::sync::Notify::new());
        let peer = (!exempt).then_some(peer);
        open.live.insert(
            id,
            Live {
                peer,
                network,
                shed: Arc::clone(&shed),
                shedding: false,
            },
        );
        if let Some(peer) = peer {
            *open.per_peer.entry(peer).or_insert(0) += 1;
        }
        *open.per_network.entry(network).or_insert(0) += 1;
        Some(ConnectionSlot {
            table: Arc::clone(table),
            id,
            shed,
        })
    }
}

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        let mut open = lock(&self.table.open);
        let Some(live) = open.live.remove(&self.id) else {
            return;
        };
        if live.shedding {
            open.shedding -= 1;
            return;
        }
        if let Some(peer) = live.peer {
            decrement(&mut open.per_peer, peer);
        }
        decrement(&mut open.per_network, live.network);
    }
}

// ---------------------------------------------------------------------------
// The archive slot
// ---------------------------------------------------------------------------

/// How long a client refused the archive slot keeps its claim on a coming
/// turn, and how long a served turn is remembered. Above the bootstrap
/// client's longest wait between retries (20 s), so a retrying node keeps its
/// place in line.
pub const COLD_TURN_MEMORY: Duration = Duration::from_secs(30);

/// Clients remembered by [`ColdTurns`]; past it a newly refused client is not
/// queued (it still gets a turn when nobody waits ahead of it).
const COLD_TURN_CLIENTS: usize = 4096;

/// The one archive fetch the node runs at a time, shared in turns.
///
/// A fetch may decode a 512 MiB archive or wait minutes on a remote gateway,
/// so it is never queued: a second caller is refused. Refusal alone let one
/// client that asked again the moment its fetch ended hold the slot for good.
/// So the slot goes to the client served longest ago among those waiting: a
/// client served more recently than another that was refused and is still
/// waiting yields to it. A client alone is never held back.
#[derive(Debug, Default)]
pub struct ColdTurns {
    turns: Mutex<Turns>,
}

#[derive(Debug, Default)]
struct Turns {
    running: bool,
    clients: HashMap<IpAddr, Turn>,
}

#[derive(Debug, Default, Clone, Copy)]
struct Turn {
    served: Option<Instant>,
    refused: Option<Instant>,
}

impl Turn {
    fn recent(at: Option<Instant>, now: Instant) -> bool {
        at.is_some_and(|at| now.saturating_duration_since(at) < COLD_TURN_MEMORY)
    }

    /// Refused recently and not served since.
    fn waiting(&self, now: Instant) -> bool {
        Self::recent(self.refused, now) && self.served < self.refused
    }
}

/// A turn on the archive slot; released on drop.
#[derive(Debug)]
pub struct ColdTurn<'a> {
    owner: &'a ColdTurns,
    client: IpAddr,
}

impl ColdTurns {
    /// The archive slot for `client`, or `None` while another fetch runs or
    /// another client is owed the turn.
    pub fn claim(&self, client: IpAddr) -> Option<ColdTurn<'_>> {
        self.claim_at(client, Instant::now())
    }

    fn claim_at(&self, client: IpAddr, now: Instant) -> Option<ColdTurn<'_>> {
        let key = bucket_key(client);
        let mut turns = lock(&self.turns);
        turns
            .clients
            .retain(|_, t| Turn::recent(t.served, now) || Turn::recent(t.refused, now));
        let mine = turns.clients.get(&key).copied().unwrap_or_default();
        let owed = mine.served.is_some()
            && turns
                .clients
                .iter()
                .any(|(other, t)| *other != key && t.waiting(now) && t.served < mine.served);
        if turns.running || owed {
            if turns.clients.len() < COLD_TURN_CLIENTS || turns.clients.contains_key(&key) {
                turns.clients.entry(key).or_default().refused = Some(now);
            }
            return None;
        }
        turns.running = true;
        Some(ColdTurn {
            owner: self,
            client: key,
        })
    }
}

impl Drop for ColdTurn<'_> {
    fn drop(&mut self) {
        let mut turns = lock(&self.owner.turns);
        turns.running = false;
        turns.clients.entry(self.client).or_default().served = Some(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().expect("address")
    }

    /// A context over a one-block chain.
    fn context() -> (RpcContext, tempfile::TempDir) {
        use crate::consensus::{Chain, ChainConfig};
        use crate::core::{Block, BlockHeader};
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(crate::state::StateDB::open(dir.path()).unwrap());
        let genesis = Block::new(
            BlockHeader {
                version: 0,
                prev_hash: [0; 32],
                state_root: [0; 32],
                timestamp: 1_700_000_000,
                nonce: 0,
                difficulty_target: crate::crypto::pow::target_from_leading_zero_bits(6),
                tx_root: [0; 32],
            },
            Vec::new(),
        );
        let chain = Chain::open(
            Arc::clone(&state),
            genesis,
            ChainConfig::without_pow_verification(),
        )
        .unwrap();
        let mempool = crate::network::Mempool::new(state);
        (RpcContext::new(Arc::new(Mutex::new(chain)), mempool), dir)
    }

    #[test]
    fn node_blocking_work_runs_while_every_rpc_slot_is_stuck() {
        // Why: blocking handlers share tokio's pool with block relay and BFT
        // catch-up; unbounded, stuck reads filled it and node work queued.
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(BLOCKING_SLOTS + 1)
            .enable_all()
            .build()
            .unwrap();
        let (context, _dir) = context();
        let slots = Arc::clone(&context.blocking);
        let mut module = RpcModule::new(context);
        let gate = Arc::new(std::sync::RwLock::new(()));
        let closed = gate.write().unwrap();
        let held = Arc::clone(&gate);
        module
            .register_bounded("wait", move |_, _, _| {
                drop(held.read());
                Ok::<_, ErrorObjectOwned>(())
            })
            .unwrap();
        let module = Arc::new(module);

        let calls: Vec<_> = (0..4 * BLOCKING_SLOTS)
            .map(|_| {
                let module = Arc::clone(&module);
                runtime.spawn(async move {
                    module.call::<_, ()>("wait", jsonrpsee::rpc_params![]).await
                })
            })
            .collect();
        let deadline = Instant::now() + Duration::from_secs(5);
        while slots.available_permits() > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        // Every RPC slot is stuck; the node's own blocking work still runs.
        let node_work = runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), tokio::task::spawn_blocking(|| 7)).await
        });
        assert_eq!(
            node_work.unwrap().unwrap(),
            7,
            "node work queued behind RPC"
        );

        drop(closed);
        let answers: Vec<_> = calls
            .into_iter()
            .map(|call| runtime.block_on(call).unwrap())
            .collect();
        let served = answers.iter().filter(|a| a.is_ok()).count();
        assert_eq!(served, BLOCKING_SLOTS);
        for refused in answers.iter().filter_map(|a| a.as_ref().err()) {
            assert!(refused.to_string().contains(RPC_BUSY), "{refused}");
        }
    }

    #[test]
    fn one_peer_cannot_hold_more_than_its_share_of_connections() {
        let table = Arc::new(Connections::new(1024));
        let host = ip("203.0.113.7");
        let mut held: Vec<_> = (0..MAX_CONNECTIONS_PER_PEER)
            .map(|_| Connections::claim(&table, host, false).expect("within the cap"))
            .collect();
        assert!(
            Connections::claim(&table, host, false).is_none(),
            "one past the cap"
        );
        assert!(Connections::claim(&table, ip("198.51.100.8"), false).is_some());
        // Another address in the same IPv6 /64 is the same host.
        let _v6: Vec<_> = (0..MAX_CONNECTIONS_PER_PEER)
            .map(|_| Connections::claim(&table, ip("2001:db8::1"), false).expect("within"))
            .collect();
        assert!(Connections::claim(&table, ip("2001:db8::2"), false).is_none());
        // A closed connection frees its slot.
        held.pop();
        assert!(Connections::claim(&table, host, false).is_some());
    }

    #[test]
    fn one_network_cannot_hold_more_than_its_share_from_many_peers() {
        let table = Arc::new(Connections::new(1024));
        // Every /64 of one /48 under its own per-peer cap.
        let _held: Vec<_> = (0..MAX_CONNECTIONS_PER_NETWORK)
            .map(|i| {
                let peer = ip(&format!("2001:db8:7:{:x}::1", i / MAX_CONNECTIONS_PER_PEER));
                Connections::claim(&table, peer, false).expect("within the network cap")
            })
            .collect();
        assert!(Connections::claim(&table, ip("2001:db8:7:ff::1"), false).is_none());
        assert!(Connections::claim(&table, ip("2001:db8:8::1"), false).is_some());
        // And an IPv4 /24 likewise.
        let _v4: Vec<_> = (0..MAX_CONNECTIONS_PER_NETWORK)
            .map(|i| {
                let peer = ip(&format!("192.0.2.{}", i / MAX_CONNECTIONS_PER_PEER));
                Connections::claim(&table, peer, false).expect("within the network cap")
            })
            .collect();
        assert!(Connections::claim(&table, ip("192.0.2.200"), false).is_none());
    }

    #[tokio::test]
    async fn a_full_table_sheds_the_heaviest_network_and_serves_the_newcomer() {
        let table = Arc::new(Connections::new(8));
        // One network holds six slots, another two: the table is full.
        let heavy: Vec<_> = (0..6)
            .map(|i| Connections::claim(&table, ip(&format!("192.0.2.{i}")), false).unwrap())
            .collect();
        let light: Vec<_> = (0..2)
            .map(|i| Connections::claim(&table, ip(&format!("198.51.100.{i}")), false).unwrap())
            .collect();
        let newcomer = Connections::claim(&table, ip("203.0.113.1"), false);
        assert!(newcomer.is_some(), "an honest newcomer is served when full");
        // The oldest connection of the heaviest network was told to close.
        tokio::time::timeout(Duration::from_secs(1), heavy[0].shed())
            .await
            .expect("the oldest heavy connection is shed");
        assert!(
            tokio::time::timeout(Duration::from_millis(50), light[0].shed())
                .await
                .is_err(),
            "the light network keeps its connections"
        );
        // Shed but not yet closed connections are bounded: an eighth over.
        let extra = Connections::claim(&table, ip("203.0.113.2"), false);
        assert!(extra.is_none(), "past the shedding headroom");
        drop(heavy);
        assert!(Connections::claim(&table, ip("203.0.113.2"), false).is_some());
        // Exempt peers skip the shares, not the limit.
        let roomy = Arc::new(Connections::new(1024));
        let tunnel: Vec<_> = (0..=MAX_CONNECTIONS_PER_NETWORK)
            .filter_map(|_| Connections::claim(&roomy, ip("127.0.0.1"), true))
            .collect();
        assert_eq!(tunnel.len(), MAX_CONNECTIONS_PER_NETWORK + 1);
    }

    #[test]
    fn the_archive_slot_goes_to_a_waiting_client_before_the_last_holder() {
        // Turns are ordered by `Instant`; a millisecond apart keeps two
        // steps from sharing one clock reading.
        let tick = || std::thread::sleep(Duration::from_millis(2));
        let turns = ColdTurns::default();
        let (a, b, c) = (ip("192.0.2.1"), ip("198.51.100.1"), ip("203.0.113.1"));
        // A client alone is never held back.
        drop(turns.claim(c).expect("free"));
        drop(turns.claim(c).expect("still free: nobody waits"));

        let first = turns.claim(a).expect("free");
        assert!(turns.claim(b).is_none(), "one fetch at a time");
        tick();
        drop(first);
        tick();
        // `b` was refused and is waiting; `a` just had a turn and yields.
        assert!(turns.claim(a).is_none(), "the last holder must yield");
        tick();
        drop(turns.claim(b).expect("the waiting client's turn"));
        tick();
        // `a` waited behind `b`, so now `a` goes, and `b` (served since) yields.
        assert!(turns.claim(b).is_none(), "b just had its turn");
        tick();
        drop(turns.claim(a).expect("a's turn"));
        // A waiter that never comes back holds nobody up past the memory.
        let later = Instant::now() + COLD_TURN_MEMORY + Duration::from_secs(1);
        assert!(turns.claim_at(a, later).is_some());
    }
}
