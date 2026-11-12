//! What the host offers, read once at start-up (HW-2).
//!
//! Every probe answers "unknown" rather than guess: an unknown value makes
//! [`super::TuningProfile::auto`] keep today's default for whatever depended on it, so
//! a host this module cannot read runs exactly as it did before it existed.
//!
//! Nothing here reaches consensus. These values size thread pools and caches,
//! and both are speed, not results (see the determinism test in
//! `crates/node/tests/tuning_profile_tests.rs`). Every probe is the standard
//! library reading `/proc` and `/sys`: no `sysinfo` crate, no `unsafe` OS call
//! (ADR-137 O-5). A probe file says whatever its environment wants (N-3), so a
//! parse failure, an out-of-range value or a "no limit" value is unknown.

use std::collections::BTreeSet;
use std::path::Path;

use serde::Serialize;

/// Kibibytes per mebibyte, for `/proc/meminfo`'s `kB` figures.
const KIB_PER_MIB: u64 = 1024;
/// Bytes per mebibyte, for cgroup limits.
const BYTES_PER_MIB: u64 = 1024 * 1024;
/// More memory than this (1 PiB) is a lie, not a machine: unknown.
pub const MAX_PLAUSIBLE_MEMORY_MIB: u64 = 1 << 30;

/// What kind of device holds the data directory, where the OS says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageKind {
    /// Non-rotational (`queue/rotational` = 0): SSD or NVMe.
    Ssd,
    /// Rotational (`queue/rotational` = 1): a spinning disk.
    Rotational,
    /// The OS did not say, or this platform is not probed.
    #[default]
    Unknown,
}

impl std::fmt::Display for StorageKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Ssd => "ssd",
            Self::Rotational => "rotational",
            Self::Unknown => "unknown",
        })
    }
}

/// The host, as far as it could be read. `Default` is "nothing known".
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct HardwareProbeProbe {
    /// x86 AVX2.
    pub avx2: bool,
    /// x86 AVX-512 Foundation.
    pub avx512f: bool,
    /// aarch64 NEON (Advanced SIMD).
    pub neon: bool,
    /// Logical CPUs this process may use.
    pub logical_cores: Option<usize>,
    /// Physical cores (distinct `physical id`/`core id` pairs on Linux).
    pub physical_cores: Option<usize>,
    /// Total memory, MiB.
    pub total_memory_mib: Option<u64>,
    /// Memory available without swapping, MiB: the smaller of `MemAvailable`
    /// and the cgroup limit (N-4).
    pub available_memory_mib: Option<u64>,
    /// The device under the data directory.
    pub storage: StorageKind,
    /// Whether a GPU adapter is present. Always `None` (not probed).
    // shortcut: the node does not link `wgpu-miner`, and adding wgpu to a core
    // crate is forbidden without an ADR; probe once a GPU-backed path exists.
    pub gpu: Option<bool>,
}

impl HardwareProbeProbe {
    /// Probes the host. Never fails: each probe that cannot answer is unknown.
    #[must_use]
    pub fn detect(data_dir: &Path) -> Self {
        let (avx2, avx512f, neon) = simd();
        let (total_memory_mib, available_memory_mib) = cap_memory(memory(), cgroup_limit_mib());
        Self {
            avx2,
            avx512f,
            neon,
            logical_cores: std::thread::available_parallelism()
                .ok()
                .map(std::num::NonZeroUsize::get),
            physical_cores: physical_cores(),
            total_memory_mib,
            available_memory_mib,
            storage: storage_kind(data_dir),
            gpu: None,
        }
    }
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn simd() -> (bool, bool, bool) {
    (
        std::arch::is_x86_feature_detected!("avx2"),
        std::arch::is_x86_feature_detected!("avx512f"),
        false,
    )
}

#[cfg(target_arch = "aarch64")]
fn simd() -> (bool, bool, bool) {
    (
        false,
        false,
        std::arch::is_aarch64_feature_detected!("neon"),
    )
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
fn simd() -> (bool, bool, bool) {
    (false, false, false)
}

/// `(MemTotal, MemAvailable)` in MiB from `/proc/meminfo` text.
#[must_use]
pub fn parse_meminfo(text: &str) -> (Option<u64>, Option<u64>) {
    let field = |name: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))
            .and_then(|rest| rest.split_whitespace().next()?.parse::<u64>().ok())
            .and_then(|kib| plausible(kib / KIB_PER_MIB))
    };
    (field("MemTotal"), field("MemAvailable"))
}

fn plausible(mib: u64) -> Option<u64> {
    (1..=MAX_PLAUSIBLE_MEMORY_MIB).contains(&mib).then_some(mib)
}

/// A cgroup memory limit file (`memory.max` in v2, `memory.limit_in_bytes` in
/// v1), in MiB. `max`, a negative or unparsable value, and v1's near-`i64::MAX`
/// "no limit" number are unknown, not a limit (ADR-137 S-18).
#[must_use]
pub fn parse_cgroup_limit(text: &str) -> Option<u64> {
    let bytes: u64 = text.trim().parse().ok()?;
    plausible(bytes / BYTES_PER_MIB)
}

/// `(total, available)` under a container limit: the smaller of each (N-4).
#[must_use]
pub fn cap_memory(
    host: (Option<u64>, Option<u64>),
    limit: Option<u64>,
) -> (Option<u64>, Option<u64>) {
    let cap = |v: Option<u64>| match (v, limit) {
        (Some(v), Some(l)) => Some(v.min(l)),
        (v, None) => v,
        // A limit with no host figure is still the most this process may use.
        (None, l) => l,
    };
    (cap(host.0), cap(host.1))
}

/// Distinct `(physical id, core id)` pairs in `/proc/cpuinfo` text; `None`
/// when the file names no core ids (most aarch64 kernels).
#[must_use]
pub fn parse_physical_cores(text: &str) -> Option<usize> {
    let mut cores = BTreeSet::new();
    let mut package = String::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        match key.trim() {
            "physical id" => value.trim().clone_into(&mut package),
            "core id" => {
                cores.insert((package.clone(), value.trim().to_owned()));
            }
            _ => {}
        }
    }
    (!cores.is_empty()).then_some(cores.len())
}

/// A sysfs `queue/rotational` value.
#[must_use]
pub fn parse_rotational(text: &str) -> StorageKind {
    match text.trim() {
        "0" => StorageKind::Ssd,
        "1" => StorageKind::Rotational,
        _ => StorageKind::Unknown,
    }
}

#[cfg(target_os = "linux")]
fn memory() -> (Option<u64>, Option<u64>) {
    std::fs::read_to_string("/proc/meminfo").map_or((None, None), |t| parse_meminfo(&t))
}

#[cfg(target_os = "linux")]
fn cgroup_limit_mib() -> Option<u64> {
    // shortcut: reads the namespace root, which is the container's own cgroup
    // under Docker and Kubernetes; a nested cgroup needs /proc/self/cgroup.
    [
        "/sys/fs/cgroup/memory.max",
        "/sys/fs/cgroup/memory/memory.limit_in_bytes",
    ]
    .iter()
    .find_map(|p| std::fs::read_to_string(p).ok())
    .and_then(|t| parse_cgroup_limit(&t))
}

#[cfg(target_os = "linux")]
fn physical_cores() -> Option<usize> {
    parse_physical_cores(&std::fs::read_to_string("/proc/cpuinfo").ok()?)
}

#[cfg(target_os = "linux")]
fn storage_kind(data_dir: &Path) -> StorageKind {
    use std::os::unix::fs::MetadataExt;
    // The data directory may not exist yet on a first start; its nearest
    // existing ancestor is on the same device unless a mount intervenes later.
    let absolute = std::path::absolute(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
    let Some(dev) = absolute
        .ancestors()
        .find_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.dev())
    else {
        return StorageKind::Unknown;
    };
    // glibc's gnu_dev_major / gnu_dev_minor.
    let major = ((dev >> 8) & 0xfff) | ((dev >> 32) & !0xfff);
    let minor = (dev & 0xff) | ((dev >> 12) & !0xff);
    let base = format!("/sys/dev/block/{major}:{minor}");
    // A partition has no queue of its own; its parent disk does.
    [
        format!("{base}/queue/rotational"),
        format!("{base}/../queue/rotational"),
    ]
    .iter()
    .find_map(|p| std::fs::read_to_string(p).ok())
    .map_or(StorageKind::Unknown, |t| parse_rotational(&t))
}

// shortcut: memory, physical cores and storage kind are read on Linux only.
// Windows and macOS report unknown and keep today's defaults (ADR-137 O-5 A);
// option B (`sysinfo`) only if a bench shows those nodes lose from it.
#[cfg(not(target_os = "linux"))]
fn memory() -> (Option<u64>, Option<u64>) {
    (None, None)
}

#[cfg(not(target_os = "linux"))]
fn cgroup_limit_mib() -> Option<u64> {
    None
}

#[cfg(not(target_os = "linux"))]
fn physical_cores() -> Option<usize> {
    None
}

#[cfg(not(target_os = "linux"))]
fn storage_kind(_data_dir: &Path) -> StorageKind {
    StorageKind::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meminfo_is_read_in_mib_and_missing_fields_are_unknown() {
        let text = "MemTotal:       16384000 kB\nMemFree: 1 kB\nMemAvailable:    8192000 kB\n";
        assert_eq!(parse_meminfo(text), (Some(16_000), Some(8_000)));
        assert_eq!(parse_meminfo("MemTotal: 2048 kB\n"), (Some(2), None));
        assert_eq!(parse_meminfo(""), (None, None));
        assert_eq!(parse_meminfo("MemTotal: lots kB\n"), (None, None));
    }

    #[test]
    fn physical_cores_count_distinct_package_core_pairs() {
        // Two packages, two cores each, two hyperthreads per core: 4 cores.
        let mut text = String::new();
        for package in 0..2 {
            for core in 0..2 {
                for _thread in 0..2 {
                    text.push_str(&format!(
                        "processor\t: x\nphysical id\t: {package}\ncore id\t\t: {core}\n\n"
                    ));
                }
            }
        }
        assert_eq!(parse_physical_cores(&text), Some(4));
        assert_eq!(parse_physical_cores("processor : 0\nBogoMIPS : 50\n"), None);
    }

    #[test]
    fn rotational_flag_maps_to_a_storage_kind() {
        assert_eq!(parse_rotational("0\n"), StorageKind::Ssd);
        assert_eq!(parse_rotational("1\n"), StorageKind::Rotational);
        assert_eq!(parse_rotational("garbage"), StorageKind::Unknown);
    }

    #[test]
    fn detection_on_this_host_does_not_panic_and_finds_cores() {
        let hw = HardwareProbe::detect(Path::new("definitely/not/here"));
        assert!(hw.logical_cores.is_some_and(|n| n >= 1));
        assert_eq!(hw.gpu, None);
    }
}
