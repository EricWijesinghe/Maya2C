//! The Linux kernel path: the XDP program, AF_XDP sockets, and their workers.
//!
//! `unsafe` lives here and nowhere else in the relay, for the reason the
//! exemption in `docs/architecture-vision.md` §7 gives: AF_XDP is memory the
//! kernel and this process share, and there is no safe interface to it in
//! `std`. Every block states the invariant it relies on, and
//! `scripts/check-unsafe.sh` refuses one that does not.

pub mod baseline;
pub mod clock;
pub mod loader;
pub mod throughput;
pub mod xsk;

use std::hash::Hash;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use maya_ebpf_net_common::packet::{self, Parsed};

use crate::error::RelayError;
use crate::ingress::{Counters, IngressReport, XdpIngressConfig, ZeroCopy};
use crate::reassembly::Reassembler;
use crate::receiver::{RelayEvent, RelayReceiver, SharedKeyBook};
use loader::Accelerator;
use xsk::XskSocket;

/// How long a worker waits for frames before checking whether to stop.
const POLL_TIMEOUT_MS: i32 = 100;

/// How long a worker pauses after a receive error, so a persistent fault logs
/// ten times a second rather than spinning a core.
const ERROR_BACKOFF: Duration = Duration::from_millis(100);

/// The running kernel receive path. Stops its workers and detaches the program
/// on drop.
pub struct XdpIngress {
    stop: Arc<AtomicBool>,
    workers: Vec<JoinHandle<()>>,
    report: IngressReport,
    accelerator: Accelerator,
}

impl std::fmt::Debug for XdpIngress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("XdpIngress")
            .field("report", &self.report)
            .field("workers", &self.workers.len())
            .finish_non_exhaustive()
    }
}

impl XdpIngress {
    /// Loads, binds, attaches, and starts one worker per queue.
    ///
    /// # Errors
    ///
    /// [`RelayError::Config`] for a bad configuration or a zero-copy
    /// requirement the driver cannot meet; otherwise the error of the step
    /// that failed. Whatever was set up before the failure is torn down.
    pub fn start<P, F>(
        config: XdpIngressConfig,
        keys: SharedKeyBook<P>,
        sink: F,
    ) -> Result<Self, RelayError>
    where
        P: Copy + Eq + Hash + Send + Sync + 'static,
        F: Fn(RelayEvent<P>) + Send + Sync + 'static,
    {
        config.validate()?;
        let mut accelerator = Accelerator::load(&config)?;

        let mut sockets = Vec::with_capacity(config.queues as usize);
        let mut zero_copy_queues = 0;
        for queue in 0..config.queues {
            let socket = XskSocket::bind(
                &config.interface,
                queue,
                config.frames_per_queue,
                config.zero_copy,
            )?;
            if socket.is_zero_copy() {
                zero_copy_queues += 1;
            } else if config.zero_copy == ZeroCopy::Required {
                return Err(RelayError::Config(format!(
                    "queue {queue} of {} bound in copy mode",
                    config.interface
                )));
            }
            accelerator.register(queue, &socket)?;
            sockets.push(socket);
        }
        accelerator.attach(&config.interface, config.attach)?;

        let mut ingress = Self {
            stop: Arc::new(AtomicBool::new(false)),
            workers: Vec::with_capacity(sockets.len()),
            report: IngressReport {
                driver_mode: accelerator.driver_mode(),
                queues: config.queues,
                zero_copy_queues,
            },
            accelerator,
        };
        let sink = Arc::new(sink);
        // One reassembler for every queue, so the memory bounds are the node's
        // and not each queue's: see `RelayReceiver::with_reassembler`.
        let reassembler = Arc::new(Mutex::new(Reassembler::new(config.reassembly)));
        for (queue, socket) in (0u32..).zip(sockets) {
            let worker = Worker {
                socket,
                receiver: RelayReceiver::with_reassembler(Arc::clone(&keys), Arc::clone(&reassembler)),
                sink: Arc::clone(&sink),
                stop: Arc::clone(&ingress.stop),
                relay_port: config.relay_port,
            };
            // On failure `ingress` drops here, stopping the workers already
            // running and detaching the program.
            let handle = std::thread::Builder::new()
                .name(format!("maya-xsk-{queue}"))
                .spawn(move || worker.run())
                .map_err(|source| RelayError::Io {
                    context: "spawn an AF_XDP receive worker",
                    source,
                })?;
            ingress.workers.push(handle);
        }
        Ok(ingress)
    }

    /// Blocks relay traffic from `ip` until `until`.
    ///
    /// # Errors
    ///
    /// [`RelayError::Ebpf`] if the blocklist is full or cannot be written.
    pub fn block(&mut self, ip: IpAddr, until: Instant) -> Result<(), RelayError> {
        let expires_ns = clock::expiry_ns(until)?;
        self.accelerator.block(ip, expires_ns)
    }

    /// Lifts a block on `ip`.
    ///
    /// # Errors
    ///
    /// [`RelayError::Ebpf`] if the blocklist cannot be written.
    pub fn unblock(&mut self, ip: IpAddr) -> Result<(), RelayError> {
        self.accelerator.unblock(ip)
    }

    /// The program's counters, summed across CPUs.
    ///
    /// # Errors
    ///
    /// [`RelayError::Ebpf`] if a lookup fails.
    pub fn counters(&self) -> Result<Counters, RelayError> {
        self.accelerator.counters()
    }

    /// How the path came up.
    #[must_use]
    pub const fn report(&self) -> IngressReport {
        self.report
    }
}

impl Drop for XdpIngress {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for worker in self.workers.drain(..) {
            if worker.join().is_err() {
                log::error!("an AF_XDP receive worker panicked");
            }
        }
    }
}

struct Worker<P, F> {
    socket: XskSocket,
    receiver: RelayReceiver<P>,
    sink: Arc<F>,
    stop: Arc<AtomicBool>,
    relay_port: u16,
}

impl<P, F> Worker<P, F>
where
    P: Copy + Eq + Hash,
    F: Fn(RelayEvent<P>),
{
    fn run(mut self) {
        while !self.stop.load(Ordering::Relaxed) {
            let now = Instant::now();
            let Self {
                socket,
                receiver,
                sink,
                relay_port,
                ..
            } = &mut self;
            let received = socket.receive(POLL_TIMEOUT_MS, |frame| {
                let Parsed::Udp(view) = packet::parse(frame) else {
                    return;
                };
                if view.destination_port != *relay_port {
                    return;
                }
                if let Ok(Some(event)) = receiver.ingest(view.payload, now) {
                    (sink.as_ref())(event);
                }
            });
            if let Err(error) = received {
                log::warn!("AF_XDP receive failed: {error}");
                std::thread::sleep(ERROR_BACKOFF);
            }
        }
    }
}
