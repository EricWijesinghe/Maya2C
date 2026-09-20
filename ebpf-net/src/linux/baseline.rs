//! Kernel-socket baselines and namespace plumbing for `benches/ebpf_bench.rs`.
//!
//! The throughput comparison is only honest if the baseline is the best the
//! kernel socket API offers, not the worst: [`BatchReceiver`] is `recvmmsg`
//! with preallocated buffers, which is what a tuned UDP receiver does.

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;

/// Bytes per baseline receive buffer. Larger than any relay datagram.
pub const BATCH_BUFFER: usize = 2048;

/// Most datagrams one call takes. Also what keeps the count passed to
/// `recvmmsg` exactly the number of headers allocated.
pub const MAX_BATCH: usize = 1_024;

/// `recvmmsg` over preallocated buffers.
pub struct BatchReceiver {
    // Field order is drop order, and `headers` points into `iovecs`, which
    // points into `buffers`: nothing dangles while any of them is alive, and
    // none of the three is ever resized after construction.
    headers: Vec<libc::mmsghdr>,
    iovecs: Vec<libc::iovec>,
    buffers: Vec<[u8; BATCH_BUFFER]>,
}

// SAFETY: the raw pointers inside `headers` and `iovecs` point only into
// buffers this struct owns, so moving the struct to another thread moves
// everything they point at with it.
unsafe impl Send for BatchReceiver {}

impl BatchReceiver {
    /// A receiver taking up to `batch` datagrams per call, clamped to
    /// `1..=MAX_BATCH`.
    #[must_use]
    pub fn new(batch: usize) -> Self {
        let mut buffers = vec![[0u8; BATCH_BUFFER]; batch.clamp(1, MAX_BATCH)];
        let mut iovecs: Vec<libc::iovec> = buffers
            .iter_mut()
            .map(|buffer| libc::iovec {
                iov_base: buffer.as_mut_ptr().cast(),
                iov_len: BATCH_BUFFER,
            })
            .collect();
        let headers = iovecs
            .iter_mut()
            .map(|iovec| {
                // SAFETY: `mmsghdr` is integers and raw pointers, for which
                // all-zero is valid; some targets give `msghdr` private padding
                // fields, so it cannot be built literally.
                let mut header: libc::mmsghdr = unsafe { std::mem::zeroed() };
                header.msg_hdr.msg_iov = iovec;
                header.msg_hdr.msg_iovlen = 1;
                header
            })
            .collect();
        Self {
            headers,
            iovecs,
            buffers,
        }
    }

    /// Receives what is queued, without blocking, handing each payload to
    /// `each`. Returns how many datagrams were received.
    ///
    /// # Errors
    ///
    /// The OS error, except `EAGAIN`, which is `Ok(0)`.
    pub fn receive(
        &mut self,
        socket: &impl AsRawFd,
        mut each: impl FnMut(&[u8]),
    ) -> io::Result<usize> {
        debug_assert_eq!(self.iovecs.len(), self.headers.len());
        // At most MAX_BATCH, so this cannot fail; if it somehow did, refusing is
        // the only answer that cannot overstate the buffers.
        let vlen = libc::c_uint::try_from(self.headers.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "batch exceeds c_uint"))?;
        // SAFETY: every header points at one iovec, which points at one owned
        // buffer of `iov_len` bytes; all are alive for the call, and `vlen`
        // does not exceed the number of headers.
        let received = unsafe {
            libc::recvmmsg(
                socket.as_raw_fd(),
                self.headers.as_mut_ptr(),
                vlen,
                libc::MSG_DONTWAIT,
                std::ptr::null_mut(),
            )
        };
        if received < 0 {
            let error = io::Error::last_os_error();
            return if error.kind() == io::ErrorKind::WouldBlock {
                Ok(0)
            } else {
                Err(error)
            };
        }
        let count = usize::try_from(received).unwrap_or(0);
        for (header, buffer) in self.headers.iter().zip(&self.buffers).take(count) {
            let len = (header.msg_len as usize).min(BATCH_BUFFER);
            each(&buffer[..len]);
        }
        Ok(count)
    }
}

/// Moves the calling thread into the network namespace `name`
/// (`/var/run/netns/<name>`, as `ip netns add` creates).
///
/// Only the calling thread moves; the benchmark runs its traffic generator on
/// a thread of its own for exactly that reason.
///
/// # Errors
///
/// `InvalidInput` for a name that is not a single path component, or the OS
/// error from opening the namespace or `setns`.
pub fn enter_netns(name: &str) -> io::Result<()> {
    if name.is_empty() || name.contains('/') || name == "." || name == ".." {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name:?} is not a namespace name"),
        ));
    }
    let namespace = File::open(Path::new("/var/run/netns").join(name))?;
    // SAFETY: `namespace` is an open namespace descriptor for the call's
    // duration, and setns affects only the calling thread.
    let rc = unsafe { libc::setns(namespace.as_raw_fd(), libc::CLONE_NEWNET) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
