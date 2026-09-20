//! Starting the kernel receive path.
//!
//! [`XdpIngress::start`] loads the XDP program, attaches it to an interface,
//! binds one AF_XDP socket per receive queue, and runs one worker thread per
//! socket feeding a [`crate::RelayReceiver`]. The same signatures exist on every
//! platform; off Linux, or without the `xdp` feature, `start` validates the
//! configuration and returns [`RelayError::Unsupported`]. A node therefore
//! needs no `cfg` of its own, and falls back to a kernel UDP socket.

use std::net::IpAddr;
use std::path::PathBuf;

use maya_ebpf_net_common::maps::{COUNTER_SLOTS, QUEUE_CAPACITY};
use maya_ebpf_net_common::rate::RateLimit;

use crate::error::RelayError;
use crate::reassembly::Limits;

/// Where the XDP program runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachMode {
    /// In the driver. Required for zero-copy.
    Driver,
    /// In the kernel's generic hook, after an `sk_buff` exists. Works on any
    /// interface, veth included, and saves far less.
    Generic,
    /// The driver if it supports XDP, otherwise generic.
    DriverOrGeneric,
}

/// Whether AF_XDP sockets share frames with the driver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZeroCopy {
    /// Fail to start unless every queue binds zero-copy.
    Required,
    /// Zero-copy where the driver supports it, copy mode elsewhere.
    Preferred,
    /// Copy mode.
    Off,
}

/// How the path actually came up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IngressReport {
    /// Whether the program runs in the driver.
    pub driver_mode: bool,
    /// Queues served.
    pub queues: u32,
    /// Queues whose socket bound zero-copy.
    pub zero_copy_queues: u32,
}

/// The kernel program's counters, indexed by [`maya_ebpf_net_common::Counter`]
/// slot and summed across CPUs.
pub type Counters = [u64; COUNTER_SLOTS as usize];

/// What to start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XdpIngressConfig {
    /// Interface name, e.g. `eth0`.
    pub interface: String,
    /// Receive queues to serve, `0..queues`. Relay traffic on any other queue
    /// is dropped by the program (`redirect_failed`), so this must cover every
    /// queue RSS can steer relay traffic to.
    pub queues: u32,
    /// The compiled XDP object (`ebpf-net/programs`).
    pub object: PathBuf,
    /// Where the program runs.
    pub attach: AttachMode,
    /// Zero-copy policy.
    pub zero_copy: ZeroCopy,
    /// UDP port of relay traffic.
    pub relay_port: u16,
    /// Per-source admission in the kernel.
    pub limit: RateLimit,
    /// UMEM frames per queue. A power of two.
    pub frames_per_queue: u32,
    /// Reassembly bounds per worker.
    pub reassembly: Limits,
}

/// Fewest UMEM frames per queue: below this a burst of one block's chunks
/// overruns the ring before a worker is scheduled.
pub const MIN_FRAMES_PER_QUEUE: u32 = 64;

/// Most UMEM frames per queue: 2 GiB of pinned memory at 2,048 bytes a frame.
pub const MAX_FRAMES_PER_QUEUE: u32 = 1 << 20;

impl XdpIngressConfig {
    /// Defaults for one queue of `interface`.
    #[must_use]
    pub fn new(interface: impl Into<String>, object: impl Into<PathBuf>, relay_port: u16) -> Self {
        Self {
            interface: interface.into(),
            queues: 1,
            object: object.into(),
            attach: AttachMode::DriverOrGeneric,
            zero_copy: ZeroCopy::Preferred,
            relay_port,
            limit: RateLimit::DEFAULT,
            frames_per_queue: 4_096,
            reassembly: Limits::DEFAULT,
        }
    }

    /// Checks everything that can be checked without a kernel.
    ///
    /// # Errors
    ///
    /// [`RelayError::Config`] naming the first problem.
    pub fn validate(&self) -> Result<(), RelayError> {
        let problem = if self.interface.is_empty() {
            Some("an interface name is required".to_string())
        } else if self.relay_port == 0 {
            Some("the relay port cannot be 0: the program reads 0 as disabled".to_string())
        } else if self.queues == 0 || self.queues > QUEUE_CAPACITY {
            Some(format!(
                "queues must be 1..={QUEUE_CAPACITY}, not {}",
                self.queues
            ))
        } else if !self.frames_per_queue.is_power_of_two()
            || !(MIN_FRAMES_PER_QUEUE..=MAX_FRAMES_PER_QUEUE).contains(&self.frames_per_queue)
        {
            Some(format!(
                "frames per queue must be a power of two in {MIN_FRAMES_PER_QUEUE}..={MAX_FRAMES_PER_QUEUE}, not {}",
                self.frames_per_queue
            ))
        } else if self.zero_copy == ZeroCopy::Required && self.attach == AttachMode::Generic {
            Some("zero-copy requires the program to run in the driver".to_string())
        } else if self.limit.burst == 0 {
            Some("a rate limit with no burst admits nothing".to_string())
        } else {
            None
        };
        problem.map_or(Ok(()), |message| Err(RelayError::Config(message)))
    }
}

/// Whether an address is one the kernel blocklist can hold: IPv4-mapped IPv6
/// is folded to IPv4, because that is how the packet arrives on the wire.
#[must_use]
pub fn blocklist_address(ip: IpAddr) -> IpAddr {
    ip.to_canonical()
}

#[cfg(all(target_os = "linux", feature = "xdp"))]
pub use crate::linux::XdpIngress;

#[cfg(not(all(target_os = "linux", feature = "xdp")))]
pub use unavailable::XdpIngress;

#[cfg(not(all(target_os = "linux", feature = "xdp")))]
mod unavailable {
    use std::convert::Infallible;
    use std::hash::Hash;
    use std::net::IpAddr;
    use std::time::Instant;

    use super::{Counters, IngressReport, XdpIngressConfig};
    use crate::error::RelayError;
    use crate::receiver::{RelayEvent, SharedKeyBook};

    #[cfg(target_os = "linux")]
    const REASON: &str = "this build was made without the `xdp` feature";
    #[cfg(not(target_os = "linux"))]
    const REASON: &str = "AF_XDP exists only on Linux";

    /// The kernel receive path. Never constructed in this build.
    #[derive(Debug)]
    pub struct XdpIngress {
        never: Infallible,
    }

    impl XdpIngress {
        /// Validates `config`, then reports that the path is unavailable.
        ///
        /// # Errors
        ///
        /// [`RelayError::Config`] for a bad configuration, otherwise always
        /// [`RelayError::Unsupported`].
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
            drop((keys, sink));
            Err(RelayError::Unsupported(REASON))
        }

        /// Unreachable: no value of this type exists.
        ///
        /// # Errors
        ///
        /// Never.
        pub fn block(&mut self, _ip: IpAddr, _until: Instant) -> Result<(), RelayError> {
            match self.never {}
        }

        /// Unreachable: no value of this type exists.
        ///
        /// # Errors
        ///
        /// Never.
        pub fn unblock(&mut self, _ip: IpAddr) -> Result<(), RelayError> {
            match self.never {}
        }

        /// Unreachable: no value of this type exists.
        ///
        /// # Errors
        ///
        /// Never.
        pub fn counters(&self) -> Result<Counters, RelayError> {
            match self.never {}
        }

        /// Unreachable: no value of this type exists.
        #[must_use]
        pub const fn report(&self) -> IngressReport {
            match self.never {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> XdpIngressConfig {
        XdpIngressConfig::new("eth0", "/nonexistent/maya-relay-xdp", 30_334)
    }

    #[test]
    fn the_defaults_validate() {
        config().validate().unwrap();
    }

    #[test]
    fn each_unusable_setting_is_named() {
        type Breaker = fn(&mut XdpIngressConfig);
        let cases: [(Breaker, &str); 6] = [
            (|c| c.interface.clear(), "interface"),
            (|c| c.relay_port = 0, "relay port"),
            (|c| c.queues = QUEUE_CAPACITY + 1, "queues"),
            (|c| c.frames_per_queue = 1000, "power of two"),
            (
                |c| {
                    c.zero_copy = ZeroCopy::Required;
                    c.attach = AttachMode::Generic;
                },
                "zero-copy",
            ),
            (|c| c.limit.burst = 0, "burst"),
        ];
        for (break_it, expected) in cases {
            let mut broken = config();
            break_it(&mut broken);
            let message = broken.validate().unwrap_err().to_string();
            assert!(message.contains(expected), "{message}");
        }
    }

    #[test]
    fn ipv4_mapped_addresses_are_blocked_as_ipv4() {
        let mapped: IpAddr = "::ffff:192.0.2.7".parse().unwrap();
        assert_eq!(
            blocklist_address(mapped),
            "192.0.2.7".parse::<IpAddr>().unwrap()
        );
    }

    #[cfg(not(all(target_os = "linux", feature = "xdp")))]
    #[test]
    fn without_the_kernel_path_start_reports_unsupported_after_validating() {
        use crate::receiver::KeyBook;
        let keys = KeyBook::<u8>::shared();
        let unsupported = XdpIngress::start(config(), keys.clone(), |_| {});
        assert!(matches!(unsupported, Err(RelayError::Unsupported(_))));
        let mut bad = config();
        bad.relay_port = 0;
        assert!(matches!(
            XdpIngress::start(bad, keys, |_| {}),
            Err(RelayError::Config(_))
        ));
    }
}
