//! Receive throughput of the kernel socket API against AF_XDP, and a user-space
//! drop against `XDP_DROP`, over a veth pair. Driven by `benches/ebpf_bench.rs`.
//!
//! Nothing here asserts a ratio. Each measurement reports what was received,
//! what the generator sent in the same window, and whether the generator was
//! the bottleneck — in which case the receive figure is a lower bound on the
//! receiver, and a ratio between two such figures means nothing.
//!
//! The baselines are the best the socket API offers, not the worst: `recvmmsg`
//! in batches of 64 with preallocated buffers. Comparing AF_XDP with one
//! `recv` per datagram would flatter it.

use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use maya_ebpf_net_common::header::{CHUNK_LEN, RelayHeader};
use maya_ebpf_net_common::maps::Counter;
use maya_ebpf_net_common::rate::RateLimit;

use super::baseline::{BatchReceiver, enter_netns};
use super::clock;
use super::loader::Accelerator;
use super::xsk::XskSocket;
use crate::error::RelayError;
use crate::ingress::{AttachMode, XdpIngressConfig, ZeroCopy};

/// Datagrams per `recvmmsg` call in the batched baseline.
const BATCH: usize = 64;

/// Frames in the AF_XDP measurement's UMEM.
const FRAMES: u32 = 4_096;

/// How long the generator runs before counting starts, so a measurement does
/// not include ARP resolution and cold caches.
const WARM_UP: Duration = Duration::from_millis(500);

/// The generator is called the bottleneck when the receiver took at least this
/// share of what it sent.
const GENERATOR_LIMITED_PERCENT: u64 = 90;

/// Where and how long to measure.
#[derive(Clone, Debug)]
pub struct ThroughputConfig {
    /// Host end of the veth pair, where the program attaches.
    pub interface: String,
    /// Namespace holding the peer end, where the generator runs.
    pub namespace: String,
    /// Address of the host end.
    pub local: Ipv4Addr,
    /// Address of the peer end: the generator's source.
    pub peer: Ipv4Addr,
    /// UDP port.
    pub port: u16,
    /// The compiled XDP object.
    pub object: PathBuf,
    /// Counting window per measurement.
    pub duration: Duration,
    /// Generator threads.
    pub senders: usize,
    /// Where the program runs.
    pub attach: AttachMode,
    /// AF_XDP copy policy.
    pub zero_copy: ZeroCopy,
}

/// One measurement.
#[derive(Clone, Debug)]
pub struct Measurement {
    /// What was measured.
    pub mode: &'static str,
    /// Datagrams received — or, for a drop, dropped — in the window.
    pub received: u64,
    /// Datagrams the generator sent in the same window.
    pub sent: u64,
    /// The window.
    pub elapsed: Duration,
    /// How the path came up: attach mode, copy mode.
    pub note: String,
}

impl Measurement {
    /// Datagrams per second.
    #[must_use]
    pub fn per_second(&self) -> f64 {
        #[allow(clippy::cast_precision_loss)] // a rate, printed to three figures
        let received = self.received as f64;
        received / self.elapsed.as_secs_f64().max(f64::EPSILON)
    }

    /// Whether the generator, not the receiver, set the rate.
    #[must_use]
    pub fn generator_limited(&self) -> bool {
        self.received * 100 >= self.sent * GENERATOR_LIMITED_PERCENT
    }
}

/// Runs every measurement in turn. The program is detached between them.
///
/// # Errors
///
/// The first measurement's failure: missing privileges, a missing namespace or
/// object, or a generator thread that could not send.
pub fn run_all(config: &ThroughputConfig) -> Result<Vec<Measurement>, RelayError> {
    Ok(vec![
        udp_single(config)?,
        udp_batched(config)?,
        af_xdp(config)?,
        xdp_drop(config)?,
    ])
}

/// A structurally valid relay datagram of the largest size. The kernel checks
/// structure, not the AEAD, so this passes the program as a real chunk would.
fn relay_datagram() -> Vec<u8> {
    #[allow(clippy::cast_possible_truncation)] // 2 * 1160
    let header = RelayHeader::new(1, [7; 32], 0, (CHUNK_LEN * 2) as u32)
        .expect("a two-chunk header is valid");
    let mut datagram = header.encode().to_vec();
    datagram.resize(header.datagram_len(), 0x5A);
    datagram
}

struct Generator {
    stop: Arc<AtomicBool>,
    sent: Arc<AtomicU64>,
    threads: Vec<JoinHandle<Result<(), String>>>,
}

impl Generator {
    fn start(config: &ThroughputConfig) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let sent = Arc::new(AtomicU64::new(0));
        let payload = Arc::new(relay_datagram());
        let threads = (0..config.senders.max(1))
            .map(|_| {
                let (stop, sent, payload) =
                    (Arc::clone(&stop), Arc::clone(&sent), Arc::clone(&payload));
                let namespace = config.namespace.clone();
                let target = (config.local, config.port);
                std::thread::spawn(move || {
                    enter_netns(&namespace).map_err(|e| format!("enter {namespace}: {e}"))?;
                    let socket = UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("bind: {e}"))?;
                    socket
                        .connect(target)
                        .map_err(|e| format!("connect: {e}"))?;
                    while !stop.load(Ordering::Relaxed) {
                        match socket.send(&payload) {
                            Ok(_) => {
                                sent.fetch_add(1, Ordering::Relaxed);
                            }
                            // Nothing listening (an ICMP unreachable) or a
                            // full queue: keep offering load.
                            Err(e)
                                if matches!(
                                    e.kind(),
                                    std::io::ErrorKind::ConnectionRefused
                                        | std::io::ErrorKind::WouldBlock
                                ) => {}
                            Err(e) => return Err(format!("send: {e}")),
                        }
                    }
                    Ok(())
                })
            })
            .collect();
        Self {
            stop,
            sent,
            threads,
        }
    }

    fn sent(&self) -> u64 {
        self.sent.load(Ordering::Relaxed)
    }

    fn stop(self) -> Result<(), RelayError> {
        self.stop.store(true, Ordering::Relaxed);
        for thread in self.threads {
            thread
                .join()
                .map_err(|_| RelayError::Config("a generator thread panicked".to_string()))?
                .map_err(RelayError::Config)?;
        }
        Ok(())
    }
}

fn measure(
    mode: &'static str,
    note: String,
    config: &ThroughputConfig,
    mut receive: impl FnMut() -> Result<u64, RelayError>,
) -> Result<Measurement, RelayError> {
    let generator = Generator::start(config);
    let warm_until = Instant::now() + WARM_UP;
    while Instant::now() < warm_until {
        receive()?;
    }
    let sent_before = generator.sent();
    let start = Instant::now();
    let mut received = 0;
    while start.elapsed() < config.duration {
        received += receive()?;
    }
    let elapsed = start.elapsed();
    let sent = generator.sent() - sent_before;
    generator.stop()?;
    Ok(Measurement {
        mode,
        received,
        sent,
        elapsed,
        note,
    })
}

fn io(context: &'static str) -> impl FnOnce(std::io::Error) -> RelayError {
    move |source| RelayError::Io { context, source }
}

fn udp_single(config: &ThroughputConfig) -> Result<Measurement, RelayError> {
    let socket = UdpSocket::bind((config.local, config.port)).map_err(io("bind UDP baseline"))?;
    socket
        .set_read_timeout(Some(Duration::from_millis(10)))
        .map_err(io("UDP read timeout"))?;
    let mut buffer = [0u8; 2048];
    measure(
        "kernel UDP, recv per datagram",
        "no program".to_string(),
        config,
        || Ok(u64::from(socket.recv(&mut buffer).is_ok())),
    )
}

fn udp_batched(config: &ThroughputConfig) -> Result<Measurement, RelayError> {
    let socket = UdpSocket::bind((config.local, config.port)).map_err(io("bind UDP baseline"))?;
    let mut receiver = BatchReceiver::new(BATCH);
    measure(
        "kernel UDP, recvmmsg x64",
        "no program; also the user-space drop baseline".to_string(),
        config,
        || {
            receiver
                .receive(&socket, |_| {})
                .map(|count| count as u64)
                .map_err(io("recvmmsg"))
        },
    )
}

fn xdp_config(config: &ThroughputConfig) -> XdpIngressConfig {
    let mut xdp =
        XdpIngressConfig::new(config.interface.clone(), config.object.clone(), config.port);
    // Admission is not what is being measured.
    xdp.limit = RateLimit {
        burst: u32::MAX,
        per_second: u32::MAX,
    };
    xdp.attach = config.attach;
    xdp.zero_copy = config.zero_copy;
    xdp.frames_per_queue = FRAMES;
    xdp
}

fn af_xdp(config: &ThroughputConfig) -> Result<Measurement, RelayError> {
    let xdp = xdp_config(config);
    let mut accelerator = Accelerator::load(&xdp)?;
    let mut socket = XskSocket::bind(&config.interface, 0, FRAMES, config.zero_copy)?;
    accelerator.register(0, &socket)?;
    accelerator.attach(&config.interface, config.attach)?;
    let note = format!(
        "{} mode, {}",
        if accelerator.driver_mode() {
            "driver"
        } else {
            "generic"
        },
        if socket.is_zero_copy() {
            "zero-copy"
        } else {
            "copy"
        }
    );
    measure("AF_XDP, queue 0", note, config, || {
        socket.receive(10, |_| {}).map(|count| count as u64)
    })
}

fn xdp_drop(config: &ThroughputConfig) -> Result<Measurement, RelayError> {
    let xdp = xdp_config(config);
    let mut accelerator = Accelerator::load(&xdp)?;
    accelerator.attach(&config.interface, config.attach)?;
    let hour = 3_600 * 1_000_000_000;
    accelerator.block(IpAddr::V4(config.peer), clock::monotonic_ns()? + hour)?;
    let note = format!(
        "{} mode, source blocklisted",
        if accelerator.driver_mode() {
            "driver"
        } else {
            "generic"
        }
    );
    let slot = Counter::Blocked.slot() as usize;
    let mut last = accelerator.counters()?[slot];
    measure("XDP_DROP, blocklist", note, config, || {
        std::thread::sleep(Duration::from_millis(50));
        let now = accelerator.counters()?[slot];
        let delta = now.wrapping_sub(last);
        last = now;
        Ok(delta)
    })
}
