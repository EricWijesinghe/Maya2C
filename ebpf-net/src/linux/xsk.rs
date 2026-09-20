//! A receive-only AF_XDP socket over `libc`'s `if_xdp.h` definitions.
//!
//! # Why this is hand-written
//!
//! The crates that wrap AF_XDP link C: `xsk-rs` builds libxdp and `afxdp`
//! builds libbpf, both through `cc`. What they would do here is a few ring
//! operations over memory the kernel shares with us, and `libc` already carries
//! the kernel's struct layouts. A C toolchain in the node's graph for that is a
//! poor trade.
//!
//! # Layout
//!
//! ```text
//! UMEM        frames × FRAME_SIZE bytes, anonymous, registered with the kernel
//! fill ring   user → kernel   addresses of frames the kernel may receive into
//! RX ring     kernel → user   (address, length) of frames that were received
//! completion  kernel → user   required for any UMEM; unused, nothing is sent
//! ```
//!
//! Each ring has exactly one producer and one consumer, which is the kernel's
//! contract. This type is the only user-space producer of the fill ring and the
//! only consumer of the RX ring, and every ring operation takes `&mut self`, so
//! that holds by construction.
//!
//! # Where copies happen
//!
//! In zero-copy mode the driver receives straight into UMEM, and a frame is
//! handed to the caller as a slice of it: no copy until
//! [`crate::RelayReceiver`] decrypts the chunk into its reassembly buffer, which
//! is a copy any reassembly needs. In copy mode the kernel copies once into
//! UMEM, and the caller still sees no second copy.

use std::ffi::CString;
use std::io;
use std::mem::{MaybeUninit, size_of};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::error::RelayError;
use crate::ingress::ZeroCopy;

/// Bytes per UMEM frame. The kernel's minimum, and a power of two as aligned
/// mode requires; a relay datagram's frame is at most 1,294 bytes.
pub const FRAME_SIZE: u32 = 2048;

/// Completion ring entries. The kernel refuses a UMEM without one; nothing is
/// ever transmitted, so it stays empty.
const COMPLETION_ENTRIES: u32 = 64;

/// One `mmap`ed region, unmapped on drop.
struct Mapping {
    ptr: NonNull<libc::c_void>,
    len: usize,
}

// SAFETY: a `Mapping` exclusively owns its region; moving it to another thread
// moves that ownership, and nothing else holds the pointer.
unsafe impl Send for Mapping {}

impl Mapping {
    fn anonymous(len: usize) -> Result<Self, RelayError> {
        // SAFETY: a fresh private anonymous mapping aliases no existing memory;
        // the result is checked against MAP_FAILED below.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        Self::checked(ptr, len, "mmap UMEM")
    }

    fn ring(
        fd: RawFd,
        len: usize,
        page_offset: u64,
        context: &'static str,
    ) -> Result<Self, RelayError> {
        let offset = libc::off_t::try_from(page_offset)
            .map_err(|_| RelayError::Config(format!("{context}: offset does not fit off_t")))?;
        // SAFETY: maps the ring the kernel allocated for `fd` at its documented
        // page offset; the result is checked against MAP_FAILED below.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED | libc::MAP_POPULATE,
                fd,
                offset,
            )
        };
        Self::checked(ptr, len, context)
    }

    fn checked(
        ptr: *mut libc::c_void,
        len: usize,
        context: &'static str,
    ) -> Result<Self, RelayError> {
        if ptr == libc::MAP_FAILED {
            return Err(RelayError::last_os_error(context));
        }
        NonNull::new(ptr)
            .map(|ptr| Self { ptr, len })
            .ok_or_else(|| RelayError::Config(format!("{context} returned a null mapping")))
    }

    fn base(&self) -> *mut u8 {
        self.ptr.as_ptr().cast()
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: `ptr` and `len` are exactly what mmap returned, and every
        // borrow of the region is tied to a borrow of `self`, which has ended.
        unsafe {
            libc::munmap(self.ptr.as_ptr(), self.len);
        }
    }
}

/// One shared ring.
struct Ring {
    map: Mapping,
    producer: usize,
    consumer: usize,
    flags: usize,
    desc: usize,
    entry_size: usize,
    entries: u32,
}

impl Ring {
    fn map(
        fd: RawFd,
        offsets: &libc::xdp_ring_offset,
        entries: u32,
        entry_size: usize,
        page_offset: u64,
        context: &'static str,
    ) -> Result<Self, RelayError> {
        let field = |value: u64| {
            usize::try_from(value)
                .map_err(|_| RelayError::Config(format!("{context}: offset does not fit usize")))
        };
        let desc = field(offsets.desc)?;
        let len = desc + entries as usize * entry_size;
        let (producer, consumer, flags) = (
            field(offsets.producer)?,
            field(offsets.consumer)?,
            field(offsets.flags)?,
        );
        if [producer, consumer, flags]
            .iter()
            .any(|offset| offset + size_of::<u32>() > len)
        {
            return Err(RelayError::Config(format!(
                "{context}: the kernel's ring offsets fall outside the ring"
            )));
        }
        Ok(Self {
            map: Mapping::ring(fd, len, page_offset, context)?,
            producer,
            consumer,
            flags,
            desc,
            entry_size,
            entries,
        })
    }

    fn counter(&self, offset: usize) -> &AtomicU32 {
        // SAFETY: `offset + 4 <= len` was checked when the ring was mapped. The
        // kernel places a `u32` there (a field of `struct xdp_ring`, so aligned),
        // and `AtomicU32` has `u32`'s size and alignment. The kernel writes it
        // concurrently, which is what an atomic's interior mutability permits.
        unsafe { &*self.map.base().add(offset).cast::<AtomicU32>() }
    }

    fn producer(&self) -> &AtomicU32 {
        self.counter(self.producer)
    }

    fn consumer(&self) -> &AtomicU32 {
        self.counter(self.consumer)
    }

    fn flags(&self) -> &AtomicU32 {
        self.counter(self.flags)
    }

    /// Pointer to entry `index`, wrapped into the ring.
    fn slot(&self, index: u32) -> *mut u8 {
        let wrapped = (index & (self.entries - 1)) as usize;
        // SAFETY: `wrapped < entries`, and the mapping is `desc + entries *
        // entry_size` bytes, so the result points inside it.
        unsafe { self.map.base().add(self.desc + wrapped * self.entry_size) }
    }

    /// Entries published by the producer and not yet consumed.
    fn pending(&self) -> u32 {
        let published = self.producer().load(Ordering::Acquire);
        let consumed = self.consumer().load(Ordering::Acquire);
        published.wrapping_sub(consumed).min(self.entries)
    }
}

/// A bound, receive-only AF_XDP socket.
pub struct XskSocket {
    // Field order is drop order: the descriptor closes before the rings and
    // UMEM are unmapped, and the kernel keeps its own reference to all three
    // until the socket is released.
    fd: OwnedFd,
    rx: Ring,
    fill: Ring,
    _completion: Ring,
    umem: Mapping,
    zero_copy: bool,
    recycled: Vec<u64>,
    invalid_descs: u64,
}

impl AsRawFd for XskSocket {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

impl XskSocket {
    /// Creates a socket on `queue` of `interface` with `frames` UMEM frames.
    ///
    /// # Errors
    ///
    /// [`RelayError::Io`] naming the system call that failed — commonly
    /// `EPERM` without `CAP_NET_RAW` and `CAP_BPF`, or `EOPNOTSUPP` from a
    /// zero-copy bind the driver cannot do.
    pub fn bind(
        interface: &str,
        queue: u32,
        frames: u32,
        zero_copy: ZeroCopy,
    ) -> Result<Self, RelayError> {
        if !frames.is_power_of_two() {
            return Err(RelayError::Config(format!(
                "{frames} frames is not a power of two"
            )));
        }
        let ifindex = interface_index(interface)?;

        // SAFETY: socket(2) with constant arguments; the result is checked and
        // owned immediately below.
        let raw = unsafe { libc::socket(libc::AF_XDP, libc::SOCK_RAW | libc::SOCK_CLOEXEC, 0) };
        if raw < 0 {
            return Err(RelayError::last_os_error("socket(AF_XDP)"));
        }
        // SAFETY: `raw` is a fresh descriptor that nothing else owns.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };

        let umem_len = frames as usize * FRAME_SIZE as usize;
        let umem = Mapping::anonymous(umem_len)?;
        // The v1 layout (no `flags`), which every kernel with AF_XDP accepts;
        // newer kernels zero the fields it lacks.
        let registration = libc::xdp_umem_reg_v1 {
            addr: umem.base() as u64,
            len: umem_len as u64,
            chunk_size: FRAME_SIZE,
            headroom: 0,
        };
        set_option(&fd, libc::XDP_UMEM_REG, &registration, "XDP_UMEM_REG")?;
        set_option(&fd, libc::XDP_UMEM_FILL_RING, &frames, "XDP_UMEM_FILL_RING")?;
        set_option(
            &fd,
            libc::XDP_UMEM_COMPLETION_RING,
            &COMPLETION_ENTRIES,
            "XDP_UMEM_COMPLETION_RING",
        )?;
        set_option(&fd, libc::XDP_RX_RING, &frames, "XDP_RX_RING")?;

        let offsets = mmap_offsets(&fd)?;
        let rx = Ring::map(
            raw,
            &offsets.rx,
            frames,
            size_of::<libc::xdp_desc>(),
            libc::XDP_PGOFF_RX_RING as u64,
            "mmap RX ring",
        )?;
        let fill = Ring::map(
            raw,
            &offsets.fr,
            frames,
            size_of::<u64>(),
            libc::XDP_UMEM_PGOFF_FILL_RING,
            "mmap fill ring",
        )?;
        let completion = Ring::map(
            raw,
            &offsets.cr,
            COMPLETION_ENTRIES,
            size_of::<u64>(),
            libc::XDP_UMEM_PGOFF_COMPLETION_RING,
            "mmap completion ring",
        )?;

        let mut socket = Self {
            fd,
            rx,
            fill,
            _completion: completion,
            umem,
            zero_copy: false,
            recycled: Vec::with_capacity(frames as usize),
            invalid_descs: 0,
        };
        let every_frame: Vec<u64> = (0..u64::from(frames))
            .map(|i| i * u64::from(FRAME_SIZE))
            .collect();
        if socket.fill_with(&every_frame) != every_frame.len() {
            return Err(RelayError::Config(
                "the fill ring cannot hold every UMEM frame".to_string(),
            ));
        }
        socket.bind_to(ifindex, queue, zero_copy)?;
        Ok(socket)
    }

    /// Whether the driver shares frames with this socket.
    #[must_use]
    pub const fn is_zero_copy(&self) -> bool {
        self.zero_copy
    }

    /// RX descriptors that pointed outside UMEM. Nonzero means a kernel or
    /// driver fault, not traffic.
    #[must_use]
    pub const fn invalid_descs(&self) -> u64 {
        self.invalid_descs
    }

    /// Hands every frame received so far to `on_frame`, waiting up to
    /// `timeout_ms` if there are none. Returns how many were received.
    ///
    /// # Errors
    ///
    /// [`RelayError::Io`] if waiting fails.
    pub fn receive(
        &mut self,
        timeout_ms: i32,
        mut on_frame: impl FnMut(&[u8]),
    ) -> Result<usize, RelayError> {
        let mut available = self.rx.pending();
        if available == 0 {
            self.wait(timeout_ms)?;
            available = self.rx.pending();
            if available == 0 {
                return Ok(0);
            }
        }

        let consumer = self.rx.consumer().load(Ordering::Acquire);
        // Not cleared: addresses that did not fit the fill ring last time are
        // still frames this socket owns, and dropping them would shrink UMEM
        // for good.
        let mut recycled = std::mem::take(&mut self.recycled);
        for i in 0..available {
            let slot = self.rx.slot(consumer.wrapping_add(i));
            // SAFETY: `slot` is inside the RX ring's mapping, and the entries
            // between the consumer and producer indices were published by the
            // kernel for this consumer to read. Unaligned because the ring's
            // memory is only byte-aligned as far as this code can prove.
            let desc = unsafe { slot.cast::<libc::xdp_desc>().read_unaligned() };
            match self.frame(desc.addr, desc.len) {
                Some(frame) => {
                    on_frame(frame);
                    recycled.push(desc.addr & !u64::from(FRAME_SIZE - 1));
                }
                // A descriptor outside UMEM names no frame this socket owns,
                // so there is nothing to return; count it rather than guess.
                None => self.invalid_descs += 1,
            }
        }
        self.rx
            .consumer()
            .store(consumer.wrapping_add(available), Ordering::Release);
        // The kernel publishes its fill-ring consumer index in batches, so the
        // ring can show less room than the frames just received. What does not
        // fit stays in `recycled` and goes first next time. Treating it as an
        // error stopped the receive loop on the first burst, measured over veth.
        let written = self.fill_with(&recycled);
        recycled.drain(..written);
        self.recycled = recycled;

        if self.fill.flags().load(Ordering::Acquire) & libc::XDP_RING_NEED_WAKEUP != 0 {
            self.wait(0)?;
        }
        Ok(available as usize)
    }

    /// Kernel-side drop counters for this socket.
    ///
    /// # Errors
    ///
    /// [`RelayError::Io`] if the query fails.
    pub fn statistics(&self) -> Result<libc::xdp_statistics, RelayError> {
        let mut stats = MaybeUninit::<libc::xdp_statistics>::zeroed();
        let mut len = socklen::<libc::xdp_statistics>()?;
        // SAFETY: the buffer is a zeroed `xdp_statistics` and `len` its size;
        // the kernel writes at most `len` bytes of integers into it.
        let rc = unsafe {
            libc::getsockopt(
                self.fd.as_raw_fd(),
                libc::SOL_XDP,
                libc::XDP_STATISTICS,
                stats.as_mut_ptr().cast(),
                &mut len,
            )
        };
        if rc != 0 {
            return Err(RelayError::last_os_error("getsockopt XDP_STATISTICS"));
        }
        // SAFETY: zero-initialised and then partly or fully overwritten with
        // integers by the kernel; every bit pattern is a valid value.
        Ok(unsafe { stats.assume_init() })
    }

    fn frame(&self, addr: u64, len: u32) -> Option<&[u8]> {
        let start = usize::try_from(addr).ok()?;
        let end = start.checked_add(len as usize)?;
        if end > self.umem.len {
            return None;
        }
        // SAFETY: `[start, end)` lies inside UMEM, checked above. The kernel
        // handed this frame to user space on the RX ring and does not write it
        // again until its address is returned on the fill ring, which happens
        // only after this borrow — tied to `&self` — has ended.
        Some(unsafe { std::slice::from_raw_parts(self.umem.base().add(start), len as usize) })
    }

    /// Publishes as many of `addresses` as the fill ring has room for, in
    /// order, and returns how many that was.
    fn fill_with(&mut self, addresses: &[u64]) -> usize {
        let producer = self.fill.producer().load(Ordering::Acquire);
        let free = self.fill.entries - self.fill.pending();
        let count = u32::try_from(addresses.len()).unwrap_or(u32::MAX).min(free);
        for (i, &address) in (0u32..count).zip(addresses) {
            let slot = self.fill.slot(producer.wrapping_add(i));
            // SAFETY: `slot` is inside the fill ring's mapping. This socket is
            // the ring's only producer, and entries from `producer` up to
            // `producer + free` belong to it until the producer index is
            // published below.
            unsafe { slot.cast::<u64>().write_unaligned(address) };
        }
        self.fill
            .producer()
            .store(producer.wrapping_add(count), Ordering::Release);
        count as usize
    }

    fn bind_to(&mut self, ifindex: u32, queue: u32, policy: ZeroCopy) -> Result<(), RelayError> {
        let modes: &[u16] = match policy {
            ZeroCopy::Required => &[libc::XDP_ZEROCOPY],
            ZeroCopy::Preferred => &[libc::XDP_ZEROCOPY, libc::XDP_COPY],
            ZeroCopy::Off => &[libc::XDP_COPY],
        };
        let family = u16::try_from(libc::AF_XDP)
            .map_err(|_| RelayError::Config("AF_XDP does not fit sa_family_t".to_string()))?;
        let mut last = io::Error::other("no bind was attempted");
        for &mode in modes {
            let address = libc::sockaddr_xdp {
                sxdp_family: family,
                sxdp_flags: mode | libc::XDP_USE_NEED_WAKEUP,
                sxdp_ifindex: ifindex,
                sxdp_queue_id: queue,
                sxdp_shared_umem_fd: 0,
            };
            // SAFETY: `address` is a live `sockaddr_xdp` and the length passed
            // is exactly its size.
            let rc = unsafe {
                libc::bind(
                    self.fd.as_raw_fd(),
                    (&raw const address).cast(),
                    socklen::<libc::sockaddr_xdp>()?,
                )
            };
            if rc == 0 {
                self.zero_copy = self.query_zero_copy()?;
                return Ok(());
            }
            last = io::Error::last_os_error();
        }
        Err(RelayError::Io {
            context: "bind AF_XDP socket",
            source: last,
        })
    }

    fn query_zero_copy(&self) -> Result<bool, RelayError> {
        let mut options = libc::xdp_options { flags: 0 };
        let mut len = socklen::<libc::xdp_options>()?;
        // SAFETY: `options` is a live `xdp_options` and `len` its size.
        let rc = unsafe {
            libc::getsockopt(
                self.fd.as_raw_fd(),
                libc::SOL_XDP,
                libc::XDP_OPTIONS,
                (&raw mut options).cast(),
                &mut len,
            )
        };
        if rc != 0 {
            return Err(RelayError::last_os_error("getsockopt XDP_OPTIONS"));
        }
        Ok(options.flags & libc::XDP_OPTIONS_ZEROCOPY != 0)
    }

    fn wait(&self, timeout_ms: i32) -> Result<(), RelayError> {
        let mut pollfd = libc::pollfd {
            fd: self.fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one live `pollfd`, and the count passed is 1.
        let rc = unsafe { libc::poll(&raw mut pollfd, 1, timeout_ms) };
        if rc < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(RelayError::Io {
                    context: "poll AF_XDP socket",
                    source: error,
                });
            }
        }
        Ok(())
    }
}

fn socklen<T>() -> Result<libc::socklen_t, RelayError> {
    libc::socklen_t::try_from(size_of::<T>())
        .map_err(|_| RelayError::Config("option size exceeds socklen_t".to_string()))
}

fn set_option<T>(
    fd: &OwnedFd,
    name: libc::c_int,
    value: &T,
    context: &'static str,
) -> Result<(), RelayError> {
    let len = socklen::<T>()?;
    // SAFETY: `value` points to a live `T` of `len` bytes for the duration of
    // the call, and the kernel only copies from it.
    let rc = unsafe {
        libc::setsockopt(
            fd.as_raw_fd(),
            libc::SOL_XDP,
            name,
            (value as *const T).cast(),
            len,
        )
    };
    if rc != 0 {
        return Err(RelayError::last_os_error(context));
    }
    Ok(())
}

fn mmap_offsets(fd: &OwnedFd) -> Result<libc::xdp_mmap_offsets, RelayError> {
    let mut offsets = MaybeUninit::<libc::xdp_mmap_offsets>::zeroed();
    let expected = socklen::<libc::xdp_mmap_offsets>()?;
    let mut len = expected;
    // SAFETY: the buffer is a zeroed `xdp_mmap_offsets` and `len` its size; the
    // kernel writes at most `len` bytes.
    let rc = unsafe {
        libc::getsockopt(
            fd.as_raw_fd(),
            libc::SOL_XDP,
            libc::XDP_MMAP_OFFSETS,
            offsets.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if rc != 0 {
        return Err(RelayError::last_os_error("getsockopt XDP_MMAP_OFFSETS"));
    }
    if len != expected {
        return Err(RelayError::Config(
            "this kernel's AF_XDP rings predate the flags field (Linux 5.4)".to_string(),
        ));
    }
    // SAFETY: zero-initialised, then fully written with integers by the kernel
    // (its length was checked above); every bit pattern is a valid value.
    Ok(unsafe { offsets.assume_init() })
}

/// The kernel's index for `name`.
///
/// # Errors
///
/// [`RelayError::Config`] for a name with a NUL, [`RelayError::Io`] for an
/// interface that does not exist.
pub fn interface_index(name: &str) -> Result<u32, RelayError> {
    let c_name = CString::new(name)
        .map_err(|_| RelayError::Config(format!("interface name {name:?} contains a NUL")))?;
    // SAFETY: `c_name` is a valid NUL-terminated string for the call's duration.
    let index = unsafe { libc::if_nametoindex(c_name.as_ptr()) };
    if index == 0 {
        return Err(RelayError::last_os_error("if_nametoindex"));
    }
    Ok(index)
}
