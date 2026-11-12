//! Node hardware self-tuning (HW-2, ADR-137).
//!
//! # Speed only, never a result
//!
//! A [`TuningProfile`] sets only the knobs in ADR-137's HW-2 table: the block
//! builder's signature prewarm (threads and transactions per thread) and the
//! `RocksDB` cache, write buffer, open files and background jobs. Prewarm only
//! adds CPU `verify_at` successes to a cache the sequential pass reads in
//! order, and cache sizes do not change what `RocksDB` returns, so two nodes
//! with different profiles reach the same decisions and state roots
//! (`crates/node/tests/tuning_profile_tests.rs`). The profile is a pure integer
//! function of the [`HardwareProbe`]: no floats, no clock, no hash-map order
//! (Standing Order 4, invariant 24). It never touches a validity check, the
//! mempool, gossip or peer limits, BFT timeouts or pruning (ADR-137 deny list).
//!
//! # Off is today
//!
//! [`TuningProfile::off`] is exactly the compiled defaults, and `off` is the
//! default mode until a before/after bench exists (ADR-137 O-4). Auto keeps
//! today's value for every knob whose input the probe could not read.
//!
//! # Memory headroom
//!
//! This machine's recorded failure is commit-charge exhaustion (`os error
//! 1455`), so no profile, auto or file, sizes the block cache above
//! `available / CACHE_CEILING_DIVISOR` or the write buffer above
//! `available / WRITE_BUFFER_CEILING_DIVISOR` when available memory is known.

pub mod detect;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use detect::{HardwareProbe, StorageKind};

use crate::config::StorageConfig;
use crate::error::{NodeError, Result};

/// Transactions each prewarm thread must have before another is started.
/// The value `StateDB::prewarm_verification` hard-coded before HW-2.
pub const DEFAULT_VERIFY_BATCH: usize = 32;
/// Most prewarm threads any profile asks for.
pub const MAX_VERIFY_THREADS: usize = 64;
/// Largest verify batch a profile may set.
pub const MAX_VERIFY_BATCH: usize = 4096;
/// Smallest block cache auto picks before the memory ceiling applies.
pub const MIN_BLOCK_CACHE_MIB: usize = 64;
/// Largest block cache any profile sets.
pub const MAX_BLOCK_CACHE_MIB: usize = 16_384;
/// Smallest write buffer auto picks before the memory ceiling applies.
pub const MIN_WRITE_BUFFER_MIB: usize = 16;
/// Largest write buffer any profile sets.
pub const MAX_WRITE_BUFFER_MIB: usize = 256;
/// Fewest files a profile lets `RocksDB` hold open (`-1`, all, is also allowed).
pub const MIN_OPEN_FILES: i32 = 64;
/// Most `RocksDB` background jobs a profile sets.
pub const MAX_BACKGROUND_JOBS: i32 = 16;
/// Fewest background jobs auto sets: `RocksDB`'s own default.
pub const MIN_AUTO_BACKGROUND_JOBS: i32 = 2;
/// Auto's block cache on SSD or unknown storage: available memory / this.
pub const CACHE_DIVISOR: u64 = 8;
/// Auto's block cache on a rotational disk, where a miss costs a seek.
pub const ROTATIONAL_CACHE_DIVISOR: u64 = 4;
/// Auto's write buffer: available memory / this.
pub const WRITE_BUFFER_DIVISOR: u64 = 64;
/// No profile's block cache exceeds available memory / this.
pub const CACHE_CEILING_DIVISOR: u64 = 4;
/// No profile's write buffer exceeds available memory / this.
pub const WRITE_BUFFER_CEILING_DIVISOR: u64 = 16;

/// Which profile the node runs: `--tuning` beats `[tuning] mode`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TuningMode {
    /// Today's compiled defaults, exactly.
    Off,
    /// Derived from [`HardwareProbe::detect`]. Opt-in (ADR-137 O-4).
    Auto,
    /// The operator's own values, in `[storage]` alone (ADR-137 `manual`).
    Manual,
    /// A TOML profile file, clamped to the same bounds as auto.
    File(PathBuf),
}

impl std::str::FromStr for TuningMode {
    type Err = NodeError;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "off" => Ok(Self::Off),
            "auto" => Ok(Self::Auto),
            "manual" => Ok(Self::Manual),
            "" => Err(NodeError::Decode(
                "tuning must be off, auto, manual or a profile path".to_string(),
            )),
            path => Ok(Self::File(PathBuf::from(path))),
        }
    }
}

impl std::fmt::Display for TuningMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Off => f.write_str("off"),
            Self::Auto => f.write_str("auto"),
            Self::Manual => f.write_str("manual"),
            Self::File(path) => write!(f, "file {}", path.display()),
        }
    }
}

/// The chosen sizes, and nothing else (ADR-137 N-7: a test pins the field set).
///
/// Zero `verify_threads` means every logical core and zero `background_jobs`
/// means `RocksDB`'s default: both are what the node did before HW-2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct TuningProfile {
    /// Builder prewarm threads; 0 = every core.
    pub verify_threads: usize,
    /// Transactions per prewarm thread before another starts.
    pub verify_batch: usize,
    /// `RocksDB` block cache, MiB.
    pub block_cache_mib: usize,
    /// `RocksDB` write buffer, MiB.
    pub write_buffer_mib: usize,
    /// Files `RocksDB` may keep open; -1 = all.
    pub max_open_files: i32,
    /// `RocksDB` background flush and compaction jobs; 0 = library default.
    pub background_jobs: i32,
}

impl Default for TuningProfile {
    /// A profile file's missing keys are today's defaults.
    fn default() -> Self {
        Self::off()
    }
}

impl TuningProfile {
    /// The compiled defaults, as the node ran before HW-2.
    #[must_use]
    pub fn off() -> Self {
        let storage = StorageConfig::default();
        Self {
            verify_threads: storage.verify_threads,
            verify_batch: storage.verify_batch,
            block_cache_mib: storage.block_cache_mib,
            write_buffer_mib: storage.write_buffer_mib,
            max_open_files: storage.max_open_files,
            background_jobs: storage.background_jobs,
        }
    }

    /// Derived from `hw`; each knob whose input is unknown keeps [`Self::off`]'s.
    #[must_use]
    pub fn auto(hw: &HardwareProbe) -> Self {
        let off = Self::off();
        // One core left to the OS, BFT and the network above two cores (N-9).
        let spare = hw
            .logical_cores
            .map(|n| if n > 2 { n - 1 } else { n.max(1) });
        let (block_cache_mib, write_buffer_mib) = hw.available_memory_mib.map_or(
            (off.block_cache_mib, off.write_buffer_mib),
            |available| {
                let divisor = if hw.storage == StorageKind::Rotational {
                    ROTATIONAL_CACHE_DIVISOR
                } else {
                    CACHE_DIVISOR
                };
                (
                    mib(available / divisor).clamp(MIN_BLOCK_CACHE_MIB, MAX_BLOCK_CACHE_MIB),
                    mib(available / WRITE_BUFFER_DIVISOR)
                        .clamp(MIN_WRITE_BUFFER_MIB, MAX_WRITE_BUFFER_MIB),
                )
            },
        );
        Self {
            verify_threads: spare.map_or(off.verify_threads, |n| n.min(MAX_VERIFY_THREADS)),
            // No measurement moves the batch off 32; SIMD flags are reported only (N-10).
            verify_batch: off.verify_batch,
            block_cache_mib,
            write_buffer_mib,
            max_open_files: off.max_open_files,
            background_jobs: hw.logical_cores.map_or(off.background_jobs, |n| {
                i32::try_from(n / 2)
                    .unwrap_or(MAX_BACKGROUND_JOBS)
                    .clamp(MIN_AUTO_BACKGROUND_JOBS, MAX_BACKGROUND_JOBS)
            }),
        }
        .clamped(hw)
    }

    /// Within the compiled bounds and, when available memory is known, under
    /// the memory ceilings. Zero "default" values stay zero.
    #[must_use]
    pub fn clamped(self, hw: &HardwareProbe) -> Self {
        let ceiling = |divisor: u64| {
            hw.available_memory_mib
                .map_or(usize::MAX, |a| mib(a / divisor).max(1))
        };
        Self {
            verify_threads: self.verify_threads.min(MAX_VERIFY_THREADS),
            verify_batch: self.verify_batch.clamp(1, MAX_VERIFY_BATCH),
            block_cache_mib: self
                .block_cache_mib
                .clamp(1, MAX_BLOCK_CACHE_MIB)
                .min(ceiling(CACHE_CEILING_DIVISOR)),
            write_buffer_mib: self
                .write_buffer_mib
                .clamp(1, MAX_WRITE_BUFFER_MIB)
                .min(ceiling(WRITE_BUFFER_CEILING_DIVISOR)),
            max_open_files: if self.max_open_files < 0 {
                -1
            } else {
                self.max_open_files.max(MIN_OPEN_FILES)
            },
            background_jobs: self.background_jobs.clamp(0, MAX_BACKGROUND_JOBS),
        }
    }

    /// The profile `mode` names, for `hw`.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] if a profile file cannot be read or parsed.
    pub fn resolve(mode: &TuningMode, hw: &HardwareProbe) -> Result<Self> {
        match mode {
            TuningMode::Off | TuningMode::Manual => Ok(Self::off()),
            TuningMode::Auto => Ok(Self::auto(hw)),
            TuningMode::File(path) => Ok(Self::from_file(path)?.clamped(hw)),
        }
    }

    fn from_file(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| NodeError::Decode(format!("reading {}: {e}", path.display())))?;
        toml::from_str(&text).map_err(|e| NodeError::Decode(format!("parsing tuning profile: {e}")))
    }

    /// `configured` with this profile's sizes, except where the operator set a
    /// `[storage]` value away from its default: an explicit value always wins.
    #[must_use]
    pub fn storage(&self, configured: &StorageConfig) -> StorageConfig {
        let d = StorageConfig::default();
        let c = configured;
        StorageConfig {
            block_cache_mib: pick(c.block_cache_mib, d.block_cache_mib, self.block_cache_mib),
            write_buffer_mib: pick(
                c.write_buffer_mib,
                d.write_buffer_mib,
                self.write_buffer_mib,
            ),
            max_open_files: pick(c.max_open_files, d.max_open_files, self.max_open_files),
            verify_threads: pick(c.verify_threads, d.verify_threads, self.verify_threads),
            verify_batch: pick(c.verify_batch, d.verify_batch, self.verify_batch),
            background_jobs: pick(c.background_jobs, d.background_jobs, self.background_jobs),
        }
    }

    /// The profile actually in force once `storage` overrides are applied.
    #[must_use]
    pub fn from_storage(storage: &StorageConfig) -> Self {
        Self {
            verify_threads: storage.verify_threads,
            verify_batch: storage.verify_batch,
            block_cache_mib: storage.block_cache_mib,
            write_buffer_mib: storage.write_buffer_mib,
            max_open_files: storage.max_open_files,
            background_jobs: storage.background_jobs,
        }
    }
}

fn pick<T: PartialEq>(set: T, default: T, tuned: T) -> T {
    if set == default { tuned } else { set }
}

fn mib(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

/// What the node chose and why: the startup line and `get_node_tuning`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TuningReport {
    /// `off`, `auto`, `manual` or `file <path>`.
    pub mode: String,
    /// The host as probed.
    pub hardware: HardwareProbe,
    /// The sizes in force, after any `[storage]` override.
    pub profile: TuningProfile,
}

impl std::fmt::Display for TuningReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (p, hw) = (&self.profile, &self.hardware);
        let or_unknown = |v: Option<String>| v.unwrap_or_else(|| "unknown".to_string());
        write!(
            f,
            "tuning: {} - prewarm {} threads x {}, block cache {} MiB, write buffer {} MiB, \
             open files {}, background jobs {}; host: {} logical / {} physical cores, \
             {} MiB available, storage {}, avx2 {}, avx512f {}, neon {}",
            self.mode,
            p.verify_threads,
            p.verify_batch,
            p.block_cache_mib,
            p.write_buffer_mib,
            p.max_open_files,
            p.background_jobs,
            or_unknown(hw.logical_cores.map(|n| n.to_string())),
            or_unknown(hw.physical_cores.map(|n| n.to_string())),
            or_unknown(hw.available_memory_mib.map(|n| n.to_string())),
            hw.storage,
            hw.avx2,
            hw.avx512f,
            hw.neon,
        )
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn modes_parse() {
        assert_eq!("off".parse::<TuningMode>().unwrap(), TuningMode::Off);
        assert_eq!("auto".parse::<TuningMode>().unwrap(), TuningMode::Auto);
        assert_eq!("manual".parse::<TuningMode>().unwrap(), TuningMode::Manual);
        assert_eq!(
            "p.toml".parse::<TuningMode>().unwrap(),
            TuningMode::File("p.toml".into())
        );
        assert!("".parse::<TuningMode>().is_err());
    }

    #[test]
    fn the_report_line_names_the_choice() {
        let report = TuningReport {
            mode: "off".into(),
            hardware: HardwareProbe::default(),
            profile: TuningProfile::off(),
        };
        let line = report.to_string();
        assert!(line.starts_with("tuning: off"), "{line}");
        assert!(line.contains("block cache 512 MiB"), "{line}");
        assert!(line.contains("storage unknown"), "{line}");
    }

    #[test]
    fn a_profile_file_is_clamped_and_strict() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tuning.toml");
        std::fs::write(
            &path,
            "verify_threads = 100000\nverify_batch = 0\nblock_cache_mib = 999999\n\
             write_buffer_mib = 0\nmax_open_files = 3\nbackground_jobs = 900\n",
        )
        .unwrap();
        let hw = HardwareProbe {
            available_memory_mib: Some(4_096),
            ..HardwareProbe::default()
        };
        let t = TuningProfile::resolve(&TuningMode::File(path.clone()), &hw).unwrap();
        assert_eq!(t.verify_threads, MAX_VERIFY_THREADS);
        assert_eq!(t.verify_batch, 1);
        assert_eq!(t.block_cache_mib, 1_024, "4,096 / 4");
        assert_eq!(t.write_buffer_mib, 1);
        assert_eq!(t.max_open_files, MIN_OPEN_FILES);
        assert_eq!(t.background_jobs, MAX_BACKGROUND_JOBS);

        std::fs::write(&path, "verify_thread = 4\n").unwrap();
        let unknown = HardwareProbe::default();
        assert!(TuningProfile::resolve(&TuningMode::File(path), &unknown).is_err());
        let absent = TuningMode::File(dir.path().join("absent"));
        assert!(TuningProfile::resolve(&absent, &unknown).is_err());
    }
}
