//! `CLOCK_MONOTONIC`, the clock `bpf_ktime_get_ns` reads.
//!
//! `std::time::Instant` is `CLOCK_MONOTONIC` on Linux too, but opaque; the
//! blocklist stores an absolute kernel timestamp, so the conversion goes
//! through the clock itself.

use std::time::Instant;

use crate::error::RelayError;

const NANOS_PER_SECOND: u64 = 1_000_000_000;

/// Nanoseconds on `CLOCK_MONOTONIC`.
///
/// # Errors
///
/// [`RelayError::Io`] if the clock cannot be read.
pub fn monotonic_ns() -> Result<u64, RelayError> {
    // SAFETY: `timespec` is plain integers, for which all-zero is valid; some
    // targets give it private padding fields, so it cannot be built literally.
    let mut now: libc::timespec = unsafe { std::mem::zeroed() };
    // SAFETY: `now` is a live `timespec` that the call writes.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &raw mut now) } != 0 {
        return Err(RelayError::last_os_error("clock_gettime(CLOCK_MONOTONIC)"));
    }
    let seconds = u64::try_from(now.tv_sec).unwrap_or(0);
    let nanos = u64::try_from(now.tv_nsec).unwrap_or(0);
    Ok(seconds.saturating_mul(NANOS_PER_SECOND).saturating_add(nanos))
}

/// The `CLOCK_MONOTONIC` time corresponding to `until`.
///
/// # Errors
///
/// As [`monotonic_ns`].
pub fn expiry_ns(until: Instant) -> Result<u64, RelayError> {
    let remaining = until.saturating_duration_since(Instant::now()).as_nanos();
    Ok(monotonic_ns()?.saturating_add(u64::try_from(remaining).unwrap_or(u64::MAX)))
}
