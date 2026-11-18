//! `cargo xtask netfault` — a TCP relay between validators that applies link
//! faults: partitions with a scheduled heal, fixed and jittered latency, loss
//! and a bandwidth cap (ADR-097).
//!
//! What it models, and what it does not:
//!
//! - **Partition** is enforced at the relay. Validators are started without
//!   `--allow-private-peers`, so the loopback addresses they announce through
//!   identify never enter Kademlia; they reach each other only through the
//!   relay addresses they were given as bootnodes. The partition scenario
//!   checks this with `get_peers` rather than assuming it.
//! - **Loss** cannot drop bytes on TCP without corrupting the stream, so a
//!   lost chunk is modelled as the retransmission timeout the application
//!   would see. It does not model reordering, corruption or duplicates.
//! - **Determinism**: every fault decision is drawn from a generator seeded
//!   per connection and direction, so a seed replays the same decisions. The
//!   OS scheduler still decides wall-clock timing.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::json;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use crate::devnet::{self, Procs, Result};
use crate::localnet;

/// Bytes read from one side in a single chunk.
const CHUNK_BYTES: usize = 16 * 1024;
/// Chunks queued per direction before the reader waits (backpressure).
const QUEUE_CHUNKS: usize = 256;
const RELAY_THREADS: usize = 2;
const NANOS_PER_SEC: u128 = 1_000_000_000;
const PER_MILLE: u64 = 1000;
/// Pause before accepting again after an accept error, such as descriptor exhaustion.
const ACCEPT_RETRY: Duration = Duration::from_millis(10);
/// Retransmission timeout of the `loss` profile; Linux's minimum RTO is 200 ms.
const RTO: Duration = Duration::from_millis(200);
const LATENCY: Duration = Duration::from_millis(300);
const JITTER: Duration = Duration::from_millis(100);
const LOSS_PER_MILLE: u16 = 100;
/// splitmix64 constants (Steele, Lea and Flood, 2014).
const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;
const MIX_1: u64 = 0xBF58_476D_1CE4_E5B9;
const MIX_2: u64 = 0x94D0_49BB_1331_11EB;
/// Seed used when none is given: "MAYA2C" in ASCII.
pub const DEFAULT_SEED: u64 = 0x4D41_5941_3243;
/// Time a partition is left to settle before heights are compared.
const SETTLE: Duration = Duration::from_secs(5);
/// How long a partitioned side is watched for finality.
const HALT_WATCH: Duration = Duration::from_secs(20);
/// Transfers timed under a link profile.
const TRANSFERS: u64 = 5;
const PAYMENT: u64 = 12_345;
/// First recipient byte value; each transfer uses the next one.
const RECIPIENT_BASE: u64 = 0x5A0;

/// The link shape applied to every relayed link, in both directions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Shape {
    /// Fixed one-way delay per chunk.
    pub latency: Duration,
    /// Uniform jitter: each chunk is delayed by `latency ± jitter`.
    pub jitter: Duration,
    /// Chance that a chunk is hit by loss, in parts per thousand.
    pub loss_permille: u16,
    /// Stall added to a chunk the loss hits: a retransmission timeout.
    pub rto: Duration,
    /// Cap on each direction in bytes per second; `None` is unlimited.
    pub bytes_per_sec: Option<u64>,
}

/// The named shapes `--netfault` and `cargo xtask netfault` use.
///
/// # Errors
///
/// An unknown profile name.
pub fn profile(name: &str) -> Result<Shape> {
    match name {
        "latency" => Ok(Shape {
            latency: LATENCY,
            jitter: JITTER,
            ..Shape::default()
        }),
        "loss" => Ok(Shape {
            loss_permille: LOSS_PER_MILLE,
            rto: RTO,
            ..Shape::default()
        }),
        other => Err(format!(
            "unknown netfault profile `{other}` (one of: latency, loss)"
        )),
    }
}

/// splitmix64: small, seedable, and enough to pick faults.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(GOLDEN);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(MIX_1);
        z = (z ^ (z >> 27)).wrapping_mul(MIX_2);
        z ^ (z >> 31)
    }

    /// A value in `0..n`; `n` of zero is treated as one.
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n.max(1)
    }
}

/// A seed for one stream, derived from the run's seed and a salt.
fn mix(seed: u64, salt: u64) -> u64 {
    Rng::new(seed ^ salt.wrapping_mul(GOLDEN)).next_u64()
}

/// How long one chunk waits in the relay: latency ± jitter, never negative,
/// plus a retransmission stall when the loss draw hits.
pub fn chunk_delay(shape: &Shape, rng: &mut Rng) -> Duration {
    let jitter = u64::try_from(shape.jitter.as_nanos()).unwrap_or(u64::MAX);
    let latency = u64::try_from(shape.latency.as_nanos()).unwrap_or(u64::MAX);
    // A uniform draw in 0..=2*jitter, which centres on the latency.
    let offset = rng.below(jitter.saturating_mul(2).saturating_add(1));
    let mut delay = latency.saturating_add(offset).saturating_sub(jitter);
    if rng.below(PER_MILLE) < u64::from(shape.loss_permille) {
        let stall = u64::try_from(shape.rto.as_nanos()).unwrap_or(u64::MAX);
        delay = delay.saturating_add(stall);
    }
    Duration::from_nanos(delay)
}

/// How long `bytes` occupy a link capped at `bytes_per_sec`.
pub fn transmit_time(bytes: usize, bytes_per_sec: Option<u64>) -> Duration {
    let Some(rate) = bytes_per_sec else {
        return Duration::ZERO;
    };
    let nanos = bytes as u128 * NANOS_PER_SEC / u128::from(rate.max(1));
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

/// The side each validator slot is on; `None` is on no side and never cut off.
pub type Sides = Vec<Option<u8>>;

/// Puts each group's members on that group's side.
pub fn sides_of(groups: &[Vec<u16>]) -> Sides {
    let len = groups
        .iter()
        .flatten()
        .map(|slot| usize::from(*slot) + 1)
        .max()
        .unwrap_or(0);
    let mut sides = vec![None; len];
    for (side, members) in groups.iter().enumerate() {
        for slot in members {
            sides[usize::from(*slot)] = Some(u8::try_from(side).unwrap_or(u8::MAX));
        }
    }
    sides
}

/// Whether a partition separates slots `a` and `b`.
pub fn is_cut(sides: &Sides, a: u16, b: u16) -> bool {
    let side = |slot: u16| sides.get(usize::from(slot)).copied().flatten();
    matches!((side(a), side(b)), (Some(x), Some(y)) if x != y)
}

type Cut = Option<Sides>;

fn severed(cut: &Cut, a: u16, b: u16) -> bool {
    cut.as_ref().is_some_and(|sides| is_cut(sides, a, b))
}

/// One relayed pair: validator `from` dials it to reach validator `to`.
#[derive(Clone, Debug)]
pub struct LinkSpec {
    pub from: u16,
    pub to: u16,
    /// Where the relay listens; port 0 takes a free port.
    pub listen: SocketAddr,
    /// The real p2p port of `to`.
    pub target: SocketAddr,
}

/// Every ordered pair from `sources` to `0..dests`, self-links excluded.
pub fn mesh(sources: &[u16], dests: u16, target: impl Fn(u16) -> SocketAddr) -> Vec<LinkSpec> {
    let mut links = Vec::new();
    for from in sources {
        for to in (0..dests).filter(|to| to != from) {
            links.push(LinkSpec {
                from: *from,
                to,
                listen: SocketAddr::from(([127, 0, 0, 1], 0)),
                target: target(to),
            });
        }
    }
    links
}

/// The multiaddr a validator dials to reach a relay address.
pub fn multiaddr(addr: SocketAddr) -> String {
    format!("/ip4/127.0.0.1/tcp/{}", addr.port())
}

/// What the relay did, for the report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    /// Connections refused because their link was cut.
    pub refused: u64,
    /// Open connections closed when their link was cut.
    pub severed: u64,
    /// Bytes delivered, both directions.
    pub bytes: u64,
}

#[derive(Debug, Default)]
struct Stats {
    refused: AtomicU64,
    severed: AtomicU64,
    bytes: AtomicU64,
}

/// One running relay: a listener per link, all sharing one partition state.
pub struct Relay {
    runtime: Runtime,
    cut: Arc<watch::Sender<Cut>>,
    /// Bumped by every partition and heal, so a stale heal timer does nothing.
    generation: Arc<AtomicU64>,
    bound: Vec<(u16, u16, SocketAddr)>,
    stats: Arc<Stats>,
}

/// The per-link state each connection task needs.
struct Link {
    from: u16,
    to: u16,
    target: SocketAddr,
    shape: Shape,
    seed: u64,
    cut: Arc<watch::Sender<Cut>>,
    stats: Arc<Stats>,
}

impl Link {
    fn is_cut(&self) -> bool {
        severed(&self.cut.borrow(), self.from, self.to)
    }
}

/// Starts one listener per link.
///
/// # Errors
///
/// A bandwidth cap of zero, or a listener that cannot bind.
pub fn start(links: &[LinkSpec], shape: Shape, seed: u64) -> Result<Relay> {
    if shape.bytes_per_sec == Some(0) {
        return Err("a bandwidth cap must be above 0 bytes per second".into());
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(RELAY_THREADS)
        .enable_all()
        .build()
        .map_err(|e| format!("relay runtime: {e}"))?;
    let (cut, _) = watch::channel(None);
    let cut = Arc::new(cut);
    let stats = Arc::new(Stats::default());
    let mut bound = Vec::with_capacity(links.len());
    for (index, spec) in links.iter().enumerate() {
        let listener = runtime
            .block_on(TcpListener::bind(spec.listen))
            .map_err(|e| {
                format!(
                    "relay {}->{} cannot listen on {}: {e}",
                    spec.from, spec.to, spec.listen
                )
            })?;
        let local = listener
            .local_addr()
            .map_err(|e| format!("relay listener: {e}"))?;
        bound.push((spec.from, spec.to, local));
        let link = Arc::new(Link {
            from: spec.from,
            to: spec.to,
            target: spec.target,
            shape,
            seed: mix(seed, index as u64),
            cut: cut.clone(),
            stats: stats.clone(),
        });
        runtime.handle().spawn(accept_loop(listener, link));
    }
    Ok(Relay {
        runtime,
        cut,
        generation: Arc::new(AtomicU64::new(0)),
        bound,
        stats,
    })
}

async fn accept_loop(listener: TcpListener, link: Arc<Link>) {
    let mut connection = 0_u64;
    loop {
        let client = match listener.accept().await {
            Ok((client, _)) => client,
            Err(_) => {
                tokio::time::sleep(ACCEPT_RETRY).await;
                continue;
            }
        };
        connection += 1;
        tokio::spawn(carry(client, link.clone(), mix(link.seed, connection)));
    }
}

/// Relays one accepted connection until either side is done or the link is cut.
async fn carry(client: TcpStream, link: Arc<Link>, seed: u64) {
    if link.is_cut() {
        link.stats.refused.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let Ok(server) = TcpStream::connect(link.target).await else {
        return;
    };
    // Nagle would add a delay of its own on top of the modelled one.
    let _ = client.set_nodelay(true);
    let _ = server.set_nodelay(true);
    let (client_r, client_w) = client.into_split();
    let (server_r, server_w) = server.into_split();
    let shape = link.shape;
    let mut forward = Rng::new(mix(seed, 1));
    let mut backward = Rng::new(mix(seed, 2));
    let stats = link.stats.clone();
    let both = async {
        tokio::join!(
            pump(client_r, server_w, shape, &mut forward, &stats),
            pump(server_r, client_w, shape, &mut backward, &stats),
        );
    };
    let mut cut = link.cut.subscribe();
    tokio::select! {
        () = both => {}
        () = wait_cut(&mut cut, link.from, link.to, &link.stats) => {}
    }
}

/// Resolves once the link is cut, counting the severed connection first: the
/// count must be visible before `select!` drops the sockets, or a reader sees
/// EOF and checks the count too early. An error means the relay is gone,
/// which also ends the connection.
async fn wait_cut(cut: &mut watch::Receiver<Cut>, from: u16, to: u16, stats: &Stats) {
    if cut.wait_for(|state| severed(state, from, to)).await.is_ok() {
        stats.severed.fetch_add(1, Ordering::Relaxed);
    }
}

/// Copies one direction. The reader stamps each chunk with its due time; the
/// writer holds it until then and until the bandwidth cap allows it.
async fn pump<R, W>(mut from: R, mut to: W, shape: Shape, rng: &mut Rng, stats: &Stats)
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let (tx, mut rx) = mpsc::channel::<(Instant, Vec<u8>)>(QUEUE_CHUNKS);
    let read = async {
        let mut buf = vec![0_u8; CHUNK_BYTES];
        let mut due = Instant::now();
        loop {
            let n = match from.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            // TCP delivers in order, so a chunk never overtakes the one before it.
            due = due.max(Instant::now() + chunk_delay(&shape, rng));
            if tx.send((due, buf[..n].to_vec())).await.is_err() {
                break;
            }
        }
    };
    let write = async {
        let mut free_at = Instant::now();
        while let Some((due, chunk)) = rx.recv().await {
            let start = due.max(free_at);
            tokio::time::sleep_until(start).await;
            if to.write_all(&chunk).await.is_err() {
                break;
            }
            free_at = start + transmit_time(chunk.len(), shape.bytes_per_sec);
            stats.bytes.fetch_add(chunk.len() as u64, Ordering::Relaxed);
        }
        let _ = to.shutdown().await;
    };
    tokio::join!(read, write);
}

impl Relay {
    /// The address validator `from` dials to reach `to`.
    pub fn listen(&self, from: u16, to: u16) -> Option<SocketAddr> {
        self.bound
            .iter()
            .find(|(f, t, _)| *f == from && *t == to)
            .map(|(_, _, addr)| *addr)
    }

    /// Cuts every link between the groups from now. `heal_after` heals it
    /// that long later, unless another partition or heal comes first.
    pub fn partition(&self, groups: &[Vec<u16>], heal_after: Option<Duration>) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.cut.send_replace(Some(sides_of(groups)));
        if let Some(after) = heal_after {
            let cut = self.cut.clone();
            let current = self.generation.clone();
            self.runtime.handle().spawn(async move {
                tokio::time::sleep(after).await;
                if current.load(Ordering::SeqCst) == generation {
                    cut.send_replace(None);
                }
            });
        }
    }

    /// Restores every link.
    pub fn heal(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.cut.send_replace(None);
    }

    pub fn counters(&self) -> Counters {
        Counters {
            refused: self.stats.refused.load(Ordering::Relaxed),
            severed: self.stats.severed.load(Ordering::Relaxed),
            bytes: self.stats.bytes.load(Ordering::Relaxed),
        }
    }
}

/// Parses `--scenario NAME` (default `all`) and `--seed N`.
///
/// # Errors
///
/// A seed that is not a number.
pub(crate) fn parse_args(args: &[String]) -> Result<(String, u64)> {
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|p| args.get(p + 1))
            .cloned()
    };
    let scenario = value("--scenario").unwrap_or_else(|| "all".to_owned());
    let seed = match value("--seed") {
        Some(v) => v.parse::<u64>().map_err(|e| format!("--seed: {e}"))?,
        None => DEFAULT_SEED,
    };
    Ok((scenario, seed))
}

/// Starts the localnet's four validators with every p2p link relayed.
fn boot(work: &Path, shape: Shape, seed: u64) -> Result<(Relay, Procs, String)> {
    devnet::build(&["maya2c-node", "l1-wallet"])?;
    if work.exists() {
        std::fs::remove_dir_all(work).map_err(|e| format!("clearing {}: {e}", work.display()))?;
    }
    localnet::ports_free()?;
    let address = localnet::setup_with(work, None, &|_| {})?;
    let nodes: Vec<u16> = (0..localnet::VALIDATORS).collect();
    let links = mesh(&nodes, localnet::VALIDATORS, |to| {
        SocketAddr::from(([127, 0, 0, 1], localnet::p2p_port(to)))
    });
    let relay = start(&links, shape, seed)?;
    let mut procs = Procs::default();
    for i in &nodes {
        procs.push(localnet::start_with(work, *i, None, Some(&relay))?);
    }
    localnet::wait_until("four nodes on one chain", || {
        localnet::agree(&nodes, &address, 3)
    })?;
    Ok((relay, procs, address))
}

fn heights(nodes: &[u16], address: &str) -> Result<Vec<u64>> {
    nodes
        .iter()
        .map(|i| localnet::tip(*i, address).map(|t| t.0))
        .collect()
}

/// The peer ids node `i` is connected to, from its `get_peers` view.
fn peers_of(i: u16) -> Result<Vec<String>> {
    let view = localnet::rpc(i, "get_peers", &json!([]))?;
    Ok(view["peers"]
        .as_array()
        .map(|peers| {
            peers
                .iter()
                .filter_map(|p| p["peer_id"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default())
}

fn peer_id(i: u16) -> Result<String> {
    let view = localnet::rpc(i, "get_peers", &json!([]))?;
    view["peer_id"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("node {i}: get_peers names no peer id"))
}

/// Sends one transfer to node 0 and returns how long until every node shows it.
fn transfer_finality(work: &Path, nodes: &[u16], k: u64) -> Result<Duration> {
    let recipient = format!("{:064x}", RECIPIENT_BASE + k);
    let started = Instant::now();
    if !devnet::send(work, &recipient, PAYMENT) {
        return Err("the transfer was refused".into());
    }
    localnet::wait_until("the transfer on every node", || {
        nodes
            .iter()
            .all(|i| localnet::balance(*i, &recipient).ok() == Some(PAYMENT))
    })?;
    Ok(started.elapsed())
}

/// Partition 2|2: neither side finalises; the heal restores finality and no fork.
fn partition(seed: u64, say: &mut Vec<String>) -> Result<()> {
    let work = devnet::root().join("target").join("netfault-partition");
    let (relay, _procs, address) = boot(&work, Shape::default(), seed)?;
    let nodes: Vec<u16> = (0..localnet::VALIDATORS).collect();
    say.push(format!(
        "boot: four validators on one chain, relay seed {seed}"
    ));
    relay.partition(&[vec![0, 1], vec![2, 3]], None);
    std::thread::sleep(SETTLE);
    let before = heights(&nodes, &address)?;
    std::thread::sleep(HALT_WATCH);
    let after = heights(&nodes, &address)?;
    if before != after {
        return Err(format!(
            "a side finalised across the 2|2 partition: heights {before:?} -> {after:?}"
        ));
    }
    say.push(format!(
        "partition 2|2: heights {after:?} unchanged for {HALT_WATCH:?} on both sides"
    ));
    let ids: Vec<String> = nodes.iter().map(|i| peer_id(*i)).collect::<Result<_>>()?;
    for i in [0_u16, 1] {
        let peers = peers_of(i)?;
        for j in [2_usize, 3] {
            if peers.contains(&ids[j]) {
                return Err(format!(
                    "node {i} is connected to node {j} across the partition"
                ));
            }
        }
    }
    let refused = relay.counters().refused;
    if refused == 0 {
        return Err("the relay refused no connection; the partition was not exercised".into());
    }
    say.push(format!(
        "no cross-partition peer in get_peers on nodes 0 and 1; relay refused {refused} connection(s)"
    ));
    relay.heal();
    let target = after.iter().max().copied().unwrap_or(0) + 3;
    let t = localnet::wait_until("finality after the heal", || {
        localnet::agree(&nodes, &address, target)
    })?;
    say.push(format!(
        "heal: all four agree on one chain at height >= {target} within {t:?}; no fork"
    ));
    Ok(())
}

/// Times transfers under one link shape and reports their median.
fn timed(name: &str, shape: Shape, seed: u64, say: &mut Vec<String>) -> Result<()> {
    let work = devnet::root()
        .join("target")
        .join(format!("netfault-{name}"));
    let (relay, _procs, _address) = boot(&work, shape, seed)?;
    let nodes: Vec<u16> = (0..localnet::VALIDATORS).collect();
    say.push(format!(
        "{name}: four validators on one chain under {shape:?}"
    ));
    let mut samples = Vec::new();
    for k in 0..TRANSFERS {
        let t = transfer_finality(&work, &nodes, k)?;
        say.push(format!(
            "{name}: transfer {} executed on all four in {t:?}",
            k + 1
        ));
        samples.push(t);
    }
    samples.sort();
    say.push(format!(
        "{name}: median of {TRANSFERS} transfers {:?}; relay carried {} bytes",
        samples[samples.len() / 2],
        relay.counters().bytes
    ));
    Ok(())
}

fn latency(seed: u64, say: &mut Vec<String>) -> Result<()> {
    timed("latency", profile("latency")?, seed, say)
}

fn loss(seed: u64, say: &mut Vec<String>) -> Result<()> {
    timed("loss", profile("loss")?, seed, say)
}

type Scenario = fn(u64, &mut Vec<String>) -> Result<()>;

const SCENARIOS: &[(&str, Scenario)] = &[
    ("partition", partition),
    ("latency", latency),
    ("loss", loss),
];

/// Writes the run's lines to `reports/netfault/<date>.md`, never overwriting.
fn write_report(lines: &[String], verdict: &str) -> Result<PathBuf> {
    let dir = devnet::root().join("reports").join("netfault");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let stamp = devnet::output(Command::new("git").args(["rev-parse", "--short", "HEAD"]))
        .unwrap_or_default();
    let path = dir.join(format!(
        "{}.md",
        crate::sweep::free_stem(&dir, &crate::sweep::today())
    ));
    let body = format!(
        "# Netfault run\n\n- commit: {}\n- validators: {} (localnet, one operator, loopback)\n- host: {} / {}\n- verdict: **{verdict}**\n\n```\n{}\n```\n",
        stamp.trim(),
        localnet::VALIDATORS,
        std::env::consts::OS,
        std::env::consts::ARCH,
        lines.join("\n")
    );
    std::fs::write(&path, body).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// Runs the netfault scenarios: `--scenario partition|latency|loss|all`.
///
/// # Errors
///
/// The first scenario whose check fails, with its logs left in place.
pub fn netfault(args: &[String]) -> Result<()> {
    let (scenario, seed) = parse_args(args)?;
    let chosen: Vec<&(&str, Scenario)> = SCENARIOS
        .iter()
        .filter(|(name, _)| scenario == "all" || scenario == *name)
        .collect();
    if chosen.is_empty() {
        let names: Vec<&str> = SCENARIOS.iter().map(|(n, _)| *n).collect();
        return Err(format!(
            "unknown scenario `{scenario}` (one of: {}, all)",
            names.join(", ")
        ));
    }
    let mut say = Vec::new();
    let mut outcome = Ok(());
    for (name, run) in chosen {
        say.push(format!("== {name} =="));
        if let Err(e) = run(seed, &mut say) {
            say.push(format!("{name}: FAIL: {e}"));
            outcome = Err(e);
            break;
        }
        say.push(format!("{name}: PASS"));
    }
    for line in &say {
        println!("netfault: {line}");
    }
    let verdict = match &outcome {
        Ok(()) => "PASS".to_owned(),
        Err(e) => format!("FAIL: {e}"),
    };
    match write_report(&say, &verdict) {
        Ok(path) => println!("netfault: report {}", path.display()),
        Err(e) => eprintln!("netfault: report not written: {e}"),
    }
    outcome
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};

    /// An echo server on a thread; returns its address.
    fn echo() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                std::thread::spawn(move || {
                    let mut conn = conn;
                    let mut buf = [0u8; 4096];
                    while let Ok(n) = conn.read(&mut buf) {
                        if n == 0 || conn.write_all(&buf[..n]).is_err() {
                            break;
                        }
                    }
                });
            }
        });
        addr
    }

    fn link(from: u16, to: u16, target: SocketAddr) -> LinkSpec {
        LinkSpec {
            from,
            to,
            listen: "127.0.0.1:0".parse().unwrap(),
            target,
        }
    }

    fn client(relay: &Relay, from: u16, to: u16) -> TcpStream {
        let addr = relay.listen(from, to).unwrap();
        let s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s
    }

    fn pattern(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn the_same_seed_draws_the_same_sequence() {
        // Why: a fault run must be replayable from its seed.
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        let xs: Vec<u64> = (0..64).map(|_| a.next_u64()).collect();
        let ys: Vec<u64> = (0..64).map(|_| b.next_u64()).collect();
        assert_eq!(xs, ys);
    }

    #[test]
    fn another_seed_draws_another_sequence() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(8);
        assert_ne!(a.next_u64(), b.next_u64());
    }

    #[test]
    fn no_shape_adds_no_delay() {
        let mut rng = Rng::new(1);
        assert_eq!(chunk_delay(&Shape::default(), &mut rng), Duration::ZERO);
    }

    #[test]
    fn jitter_keeps_each_chunk_inside_latency_plus_or_minus_jitter() {
        // Why: 300 ms ± 100 ms must mean 200..=400 ms, never outside it.
        let shape = Shape {
            latency: Duration::from_millis(300),
            jitter: Duration::from_millis(100),
            ..Shape::default()
        };
        let mut rng = Rng::new(42);
        for _ in 0..1000 {
            let d = chunk_delay(&shape, &mut rng);
            assert!(d >= Duration::from_millis(200), "{d:?} below 200 ms");
            assert!(d <= Duration::from_millis(400), "{d:?} above 400 ms");
        }
    }

    #[test]
    fn a_loss_hit_adds_one_retransmission_stall() {
        // Why: loss on TCP shows up as a retransmit delay, not missing bytes.
        let always = Shape {
            loss_permille: 1000,
            rto: Duration::from_millis(200),
            ..Shape::default()
        };
        let never = Shape {
            loss_permille: 0,
            rto: Duration::from_millis(200),
            ..Shape::default()
        };
        let mut rng = Rng::new(3);
        assert_eq!(chunk_delay(&always, &mut rng), Duration::from_millis(200));
        assert_eq!(chunk_delay(&never, &mut rng), Duration::ZERO);
    }

    #[test]
    fn a_bandwidth_cap_spaces_bytes_by_their_transmit_time() {
        assert_eq!(transmit_time(1000, Some(1000)), Duration::from_secs(1));
        assert_eq!(transmit_time(1000, None), Duration::ZERO);
    }

    #[test]
    fn a_partition_cuts_only_links_between_its_sides() {
        // Why: the 2|2 split must cut 0-2 and leave 0-1 alone.
        let sides = sides_of(&[vec![0, 1], vec![2, 3]]);
        assert!(is_cut(&sides, 0, 2));
        assert!(is_cut(&sides, 3, 1));
        assert!(!is_cut(&sides, 0, 1));
        assert!(!is_cut(&sides, 2, 3));
    }

    #[test]
    fn a_slot_on_no_side_is_never_cut_off() {
        // Why: the stolen-key twin is on no side; cutting it would be a silent extra fault.
        let sides = sides_of(&[vec![0], vec![1]]);
        assert!(!is_cut(&sides, 7, 0));
        assert!(!is_cut(&sides, 0, 7));
    }

    #[test]
    fn the_relay_passes_bytes_through_unchanged() {
        let target = echo();
        let relay = start(&[link(0, 1, target)], Shape::default(), 1).unwrap();
        let mut s = client(&relay, 0, 1);
        let data = pattern(64 * 1024);
        s.write_all(&data).unwrap();
        let mut back = vec![0u8; data.len()];
        s.read_exact(&mut back).unwrap();
        assert_eq!(back, data);
    }

    #[test]
    fn loss_stalls_chunks_but_never_corrupts_the_stream() {
        // Why: 10% loss must be liveness pressure, not byte corruption.
        let target = echo();
        let shape = Shape {
            loss_permille: 100,
            rto: Duration::from_millis(1),
            ..Shape::default()
        };
        let relay = start(&[link(0, 1, target)], shape, 9).unwrap();
        let mut s = client(&relay, 0, 1);
        let data = pattern(64 * 1024);
        s.write_all(&data).unwrap();
        let mut back = vec![0u8; data.len()];
        s.read_exact(&mut back).unwrap();
        assert_eq!(back, data);
    }

    #[test]
    fn latency_is_paid_in_each_direction() {
        // Why: a round trip crosses the link twice; sleeps never return early, so the bound is exact.
        let target = echo();
        let shape = Shape {
            latency: Duration::from_millis(150),
            ..Shape::default()
        };
        let relay = start(&[link(0, 1, target)], shape, 2).unwrap();
        let mut s = client(&relay, 0, 1);
        let started = std::time::Instant::now();
        s.write_all(b"ping").unwrap();
        let mut back = [0u8; 4];
        s.read_exact(&mut back).unwrap();
        assert!(started.elapsed() >= Duration::from_millis(300));
        assert_eq!(&back, b"ping");
    }

    #[test]
    fn a_partition_refuses_new_connections_across_it() {
        let target = echo();
        let relay = start(&[link(0, 1, target)], Shape::default(), 4).unwrap();
        relay.partition(&[vec![0], vec![1]], None);
        let mut s = client(&relay, 0, 1);
        let mut buf = [0u8; 1];
        // The relay drops the accepted socket, so the read sees EOF or a reset.
        assert!(!matches!(s.read(&mut buf), Ok(n) if n == 1));
        assert!(relay.counters().refused >= 1);
    }

    #[test]
    fn a_partition_severs_an_open_connection() {
        let target = echo();
        let relay = start(&[link(0, 1, target)], Shape::default(), 5).unwrap();
        let mut s = client(&relay, 0, 1);
        s.write_all(b"a").unwrap();
        let mut one = [0u8; 1];
        s.read_exact(&mut one).unwrap();
        relay.partition(&[vec![0], vec![1]], None);
        let _ = s.write_all(b"b");
        let mut buf = [0u8; 1];
        assert!(!matches!(s.read(&mut buf), Ok(n) if n == 1));
        assert!(relay.counters().severed >= 1);
    }

    #[test]
    fn a_scheduled_heal_restores_the_link() {
        let target = echo();
        let relay = start(&[link(0, 1, target)], Shape::default(), 6).unwrap();
        relay.partition(&[vec![0], vec![1]], Some(Duration::from_millis(200)));
        std::thread::sleep(Duration::from_millis(600));
        let mut s = client(&relay, 0, 1);
        s.write_all(b"ok").unwrap();
        let mut back = [0u8; 2];
        s.read_exact(&mut back).unwrap();
        assert_eq!(&back, b"ok");
    }

    #[test]
    fn an_old_heal_timer_does_not_undo_a_newer_partition() {
        // Why: the first partition's timer must not heal the second one early.
        let target = echo();
        let relay = start(&[link(0, 1, target)], Shape::default(), 8).unwrap();
        relay.partition(&[vec![0], vec![1]], Some(Duration::from_millis(100)));
        relay.partition(&[vec![0], vec![1]], None);
        std::thread::sleep(Duration::from_millis(400));
        let mut s = client(&relay, 0, 1);
        let mut buf = [0u8; 1];
        assert!(!matches!(s.read(&mut buf), Ok(n) if n == 1));
    }

    #[test]
    fn profiles_are_named_and_unknown_names_are_refused() {
        assert!(profile("latency").is_ok());
        assert!(profile("loss").is_ok());
        assert!(profile("no-such-profile").is_err());
    }

    #[test]
    fn the_command_line_reads_scenario_and_seed() {
        let args: Vec<String> = ["--scenario", "partition", "--seed", "7"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        assert_eq!(parse_args(&args).unwrap(), ("partition".to_owned(), 7));
        assert!(parse_args(&["--seed".to_owned(), "x".to_owned()]).is_err());
    }
}
