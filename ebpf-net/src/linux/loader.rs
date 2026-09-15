//! Loading the XDP object, its maps, and attaching it.
//!
//! The order in [`crate::linux::XdpIngress::start`] is load, bind sockets,
//! register them, *then* attach, so the program never runs with an empty socket
//! map and drops a burst of honest relay traffic as `redirect_failed`.

use std::fmt::Display;
use std::net::IpAddr;
use std::os::fd::AsRawFd;

use aya::Ebpf;
use aya::maps::{Array, HashMap, MapData, MapError, PerCpuArray, XskMap};
use aya::programs::{Xdp, XdpMode};
use maya_ebpf_net_common::maps::{self, BlockEntry, COUNTER_SLOTS, Config, Counter};

use crate::error::RelayError;
use crate::ingress::{AttachMode, Counters, XdpIngressConfig, blocklist_address};

fn ebpf_error<E: Display>(context: &'static str) -> impl FnOnce(E) -> RelayError {
    move |error| RelayError::Ebpf {
        context,
        message: error.to_string(),
    }
}

fn missing(name: &'static str) -> RelayError {
    RelayError::Ebpf {
        context: "object",
        message: format!("the XDP object has no `{name}`"),
    }
}

/// The loaded program and the maps user space writes.
pub struct Accelerator {
    block_v4: HashMap<MapData, u32, BlockEntry>,
    block_v6: HashMap<MapData, [u8; 16], BlockEntry>,
    xsks: XskMap<MapData>,
    counters: PerCpuArray<MapData, u64>,
    driver_mode: bool,
    // Last, so the program — and with it the XDP link — is dropped, and so
    // detached, after the maps it reads.
    ebpf: Ebpf,
}

impl Accelerator {
    /// Reads, verifies and configures the object. Does not attach it.
    ///
    /// # Errors
    ///
    /// [`RelayError::Io`] reading the object; [`RelayError::Ebpf`] for a
    /// missing map or program, a verifier refusal, or a map write.
    pub fn load(config: &XdpIngressConfig) -> Result<Self, RelayError> {
        let object = std::fs::read(&config.object).map_err(|source| RelayError::Io {
            context: "read the XDP object",
            source,
        })?;
        let mut ebpf = Ebpf::load(&object).map_err(ebpf_error("load object"))?;

        let config_map = ebpf.map_mut(maps::CONFIG).ok_or_else(|| missing(maps::CONFIG))?;
        let mut config_array: Array<&mut MapData, Config> =
            Array::try_from(config_map).map_err(ebpf_error("CONFIG map"))?;
        config_array
            .set(0, Config::new(config.relay_port, config.limit), 0)
            .map_err(ebpf_error("write CONFIG"))?;

        let block_v4 = HashMap::try_from(take(&mut ebpf, maps::BLOCK_V4)?).map_err(ebpf_error("BLOCK_V4 map"))?;
        let block_v6 = HashMap::try_from(take(&mut ebpf, maps::BLOCK_V6)?).map_err(ebpf_error("BLOCK_V6 map"))?;
        let xsks = XskMap::try_from(take(&mut ebpf, maps::XSKS)?).map_err(ebpf_error("XSKS map"))?;
        let counters = PerCpuArray::try_from(take(&mut ebpf, maps::COUNTERS)?).map_err(ebpf_error("COUNTERS map"))?;

        let program: &mut Xdp = ebpf
            .program_mut(maps::PROGRAM)
            .ok_or_else(|| missing(maps::PROGRAM))?
            .try_into()
            .map_err(ebpf_error("program type"))?;
        program.load().map_err(ebpf_error("verifier"))?;

        Ok(Self {
            block_v4,
            block_v6,
            xsks,
            counters,
            driver_mode: false,
            ebpf,
        })
    }

    /// Puts `socket` in the socket map at `queue`.
    ///
    /// # Errors
    ///
    /// [`RelayError::Ebpf`] if the map write fails.
    pub fn register(&mut self, queue: u32, socket: &impl AsRawFd) -> Result<(), RelayError> {
        self.xsks
            .set(queue, socket.as_raw_fd(), 0)
            .map_err(ebpf_error("register AF_XDP socket"))
    }

    /// Attaches the program to `interface`.
    ///
    /// # Errors
    ///
    /// [`RelayError::Ebpf`] with the last attach error if every permitted mode
    /// fails.
    pub fn attach(&mut self, interface: &str, mode: AttachMode) -> Result<(), RelayError> {
        let modes: &[XdpMode] = match mode {
            AttachMode::Driver => &[XdpMode::Driver],
            AttachMode::Generic => &[XdpMode::Skb],
            AttachMode::DriverOrGeneric => &[XdpMode::Driver, XdpMode::Skb],
        };
        let program: &mut Xdp = self
            .ebpf
            .program_mut(maps::PROGRAM)
            .ok_or_else(|| missing(maps::PROGRAM))?
            .try_into()
            .map_err(ebpf_error("program type"))?;
        let mut last = String::from("no attach mode was attempted");
        for &xdp_mode in modes {
            match program.attach(interface, xdp_mode) {
                Ok(_link) => {
                    self.driver_mode = xdp_mode == XdpMode::Driver;
                    return Ok(());
                }
                Err(error) => last = error.to_string(),
            }
        }
        Err(RelayError::Ebpf {
            context: "attach",
            message: last,
        })
    }

    /// Whether the program runs in the driver.
    #[must_use]
    pub const fn driver_mode(&self) -> bool {
        self.driver_mode
    }

    /// Blocks relay traffic from `ip` until `expires_ns` (`CLOCK_MONOTONIC`).
    ///
    /// # Errors
    ///
    /// [`RelayError::Ebpf`] if the map is full or the write fails.
    pub fn block(&mut self, ip: IpAddr, expires_ns: u64) -> Result<(), RelayError> {
        let entry = BlockEntry { expires_ns };
        match blocklist_address(ip) {
            IpAddr::V4(v4) => self
                .block_v4
                .insert(u32::from_ne_bytes(v4.octets()), entry, 0),
            IpAddr::V6(v6) => self.block_v6.insert(v6.octets(), entry, 0),
        }
        .map_err(ebpf_error("blocklist insert"))
    }

    /// Lifts a block. Lifting one that is not there is not an error.
    ///
    /// # Errors
    ///
    /// [`RelayError::Ebpf`] if the map operation fails.
    pub fn unblock(&mut self, ip: IpAddr) -> Result<(), RelayError> {
        let removed = match blocklist_address(ip) {
            IpAddr::V4(v4) => self.block_v4.remove(&u32::from_ne_bytes(v4.octets())),
            IpAddr::V6(v6) => self.block_v6.remove(&v6.octets()),
        };
        match removed {
            Ok(()) | Err(MapError::KeyNotFound) => Ok(()),
            Err(error) => Err(ebpf_error("blocklist remove")(error)),
        }
    }

    /// The program's counters, summed across CPUs.
    ///
    /// # Errors
    ///
    /// [`RelayError::Ebpf`] if a lookup fails.
    pub fn counters(&self) -> Result<Counters, RelayError> {
        let mut totals = [0u64; COUNTER_SLOTS as usize];
        for (total, counter) in totals.iter_mut().zip(Counter::ALL) {
            let per_cpu = self
                .counters
                .get(&counter.slot(), 0)
                .map_err(ebpf_error("read counters"))?;
            *total = per_cpu.iter().fold(0u64, |sum, value| sum.wrapping_add(*value));
        }
        Ok(totals)
    }
}

fn take(ebpf: &mut Ebpf, name: &'static str) -> Result<aya::maps::Map, RelayError> {
    ebpf.take_map(name).ok_or_else(|| missing(name))
}
