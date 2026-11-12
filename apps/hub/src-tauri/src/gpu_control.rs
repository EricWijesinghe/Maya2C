//! Vendor control behind one trait (ADR-137, HW-1). The tuner asks for two
//! knobs, a power limit and a core clock. Each backend says how its vendor
//! writes them, or refuses. Nothing here decides anything: autotune.rs does.

use std::fmt;
use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

/// The settings the tuner moves. `clock_mhz` is `None` where the vendor has
/// no clock lock, and the tuner then never touches a clock.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Knobs {
    pub power_w: f64,
    pub clock_mhz: Option<f64>,
}

/// Why a backend could not read or write. The text is what the user sees.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlError {
    /// This device, or this build, cannot be tuned at all.
    Unsupported(String),
    /// The vendor tool or sysfs refused, for example without administrator rights.
    Failed(String),
}

impl fmt::Display for ControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(why) | Self::Failed(why) => f.write_str(why),
        }
    }
}

/// One GPU's control path. `Send` so the driver can hold it between ticks.
pub trait GpuControl: Send {
    /// Names the backend in the UI.
    fn backend(&self) -> &'static str;
    /// The settings before any tuning. This is the reset target.
    fn stock(&self) -> Result<Knobs, ControlError>;
    /// Writes both knobs. On `Err` the caller resets to stock.
    fn apply(&mut self, knobs: Knobs) -> Result<(), ControlError>;
    /// Writes `stock` back.
    fn reset(&mut self, stock: Knobs) -> Result<(), ControlError>;
}

/// Writes nothing and reads nothing it cannot prove. Used where control is unsupported.
pub struct ReadOnly {
    why: &'static str,
}

impl ReadOnly {
    #[must_use]
    pub const fn new(why: &'static str) -> Self {
        Self { why }
    }
}

impl GpuControl for ReadOnly {
    fn backend(&self) -> &'static str {
        "read-only"
    }

    fn stock(&self) -> Result<Knobs, ControlError> {
        Err(ControlError::Unsupported(self.why.into()))
    }

    fn apply(&mut self, _: Knobs) -> Result<(), ControlError> {
        Err(ControlError::Unsupported(self.why.into()))
    }

    /// Nothing was changed, so there is nothing to put back.
    fn reset(&mut self, _: Knobs) -> Result<(), ControlError> {
        Ok(())
    }
}

/// Reads one `nvidia-smi --query-gpu=power.limit,clocks.max.sm` line.
#[must_use]
pub fn parse_limits(csv: &str) -> Option<Knobs> {
    let mut fields = csv.lines().next()?.split(',').map(|f| {
        f.trim()
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite() && *v > 0.0)
    });
    let power_w = fields.next()??;
    let clock_mhz = fields.next()??;
    Some(Knobs {
        power_w,
        clock_mhz: Some(clock_mhz),
    })
}

/// The first line of a tool's message, or a fixed phrase when it printed none.
fn first_line(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let line = text.lines().next().unwrap_or("").trim();
    if line.is_empty() {
        "nvidia-smi failed".into()
    } else {
        line.to_string()
    }
}

/// NVIDIA through `nvidia-smi`, the tool the telemetry already reads.
// shortcut: shells out rather than linking NVML; nvml-wrapper needs a Cargo.lock entry and a cargo vet note, so it waits for a build that can check both.
pub struct NvidiaSmi {
    index: u32,
}

const LIMITS_QUERY: &str = "--query-gpu=power.limit,clocks.max.sm";
const CSV_FORMAT: &str = "--format=csv,noheader,nounits";

impl NvidiaSmi {
    #[must_use]
    pub const fn new(index: u32) -> Self {
        Self { index }
    }

    fn run(&self, args: &[&str]) -> Result<String, ControlError> {
        let out = Command::new("nvidia-smi")
            .arg("-i")
            .arg(self.index.to_string())
            .args(args)
            .output()
            .map_err(|e| ControlError::Unsupported(format!("nvidia-smi cannot run: {e}")))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            let text = if out.stderr.is_empty() {
                &out.stdout
            } else {
                &out.stderr
            };
            Err(ControlError::Failed(first_line(text)))
        }
    }
}

impl GpuControl for NvidiaSmi {
    fn backend(&self) -> &'static str {
        "NVIDIA (nvidia-smi)"
    }

    fn stock(&self) -> Result<Knobs, ControlError> {
        let text = self.run(&[LIMITS_QUERY, CSV_FORMAT])?;
        parse_limits(&text)
            .ok_or_else(|| ControlError::Failed(format!("unreadable limits: {}", text.trim())))
    }

    fn apply(&mut self, knobs: Knobs) -> Result<(), ControlError> {
        let power = format!("{:.0}", knobs.power_w);
        self.run(&["-pl", &power])?;
        if let Some(mhz) = knobs.clock_mhz {
            let lock = format!("{mhz:.0},{mhz:.0}");
            self.run(&["-lgc", &lock])?;
        }
        Ok(())
    }

    /// Restores the power limit first, then releases the clock lock. A
    /// failure in either is reported, so the driver keeps retrying.
    fn reset(&mut self, stock: Knobs) -> Result<(), ControlError> {
        let power = format!("{:.0}", stock.power_w);
        self.run(&["-pl", &power])?;
        self.run(&["-rgc"]).map(|_| ())
    }
}

const MICROWATTS_PER_WATT: f64 = 1_000_000.0;

/// AMD through the hwmon power cap on Linux. Power only, with no clock lock,
/// so the tuner never asks for a clock on this backend.
#[cfg(target_os = "linux")]
#[allow(dead_code)] // shortcut: no AMD discovery yet (telemetry lists NVIDIA only); drop this allow when it lands
pub struct AmdSysfs {
    cap: std::path::PathBuf,
}

#[cfg(target_os = "linux")]
#[allow(dead_code)]
impl AmdSysfs {
    #[must_use]
    pub const fn new(cap: std::path::PathBuf) -> Self {
        Self { cap }
    }

    fn write_watts(&self, watts: f64) -> Result<(), ControlError> {
        let uw = format!("{:.0}", watts * MICROWATTS_PER_WATT);
        std::fs::write(&self.cap, uw)
            .map_err(|e| ControlError::Failed(format!("{}: {e}", self.cap.display())))
    }
}

#[cfg(target_os = "linux")]
impl GpuControl for AmdSysfs {
    fn backend(&self) -> &'static str {
        "AMD (sysfs power cap)"
    }

    fn stock(&self) -> Result<Knobs, ControlError> {
        let text = std::fs::read_to_string(&self.cap)
            .map_err(|e| ControlError::Failed(format!("{}: {e}", self.cap.display())))?;
        let uw: f64 = text
            .trim()
            .parse()
            .map_err(|_| ControlError::Failed(format!("unreadable power cap: {}", text.trim())))?;
        Ok(Knobs {
            power_w: uw / MICROWATTS_PER_WATT,
            clock_mhz: None,
        })
    }

    fn apply(&mut self, knobs: Knobs) -> Result<(), ControlError> {
        self.write_watts(knobs.power_w)
    }

    fn reset(&mut self, stock: Knobs) -> Result<(), ControlError> {
        self.write_watts(stock.power_w)
    }
}

/// The AMD backend for one hwmon directory: sysfs on Linux.
#[cfg(target_os = "linux")]
#[allow(dead_code)] // shortcut: no caller until AMD discovery is wired (see AmdSysfs)
#[must_use]
pub fn amd_control(hwmon: &Path) -> Box<dyn GpuControl> {
    Box::new(AmdSysfs::new(hwmon.join("power1_cap")))
}

/// Windows AMD stays read-only.
#[cfg(not(target_os = "linux"))]
#[allow(dead_code)] // shortcut: no caller until AMD discovery is wired (see AmdSysfs)
#[must_use]
pub fn amd_control(hwmon: &Path) -> Box<dyn GpuControl> {
    // shortcut: Windows AMD is read-only until a vendor control path (ADLX) is wired; revisit when Windows AMD tuning is asked for.
    let _ = hwmon;
    Box::new(ReadOnly::new("AMD tuning on Windows is read-only for now"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_power_limit_and_the_max_clock() {
        assert_eq!(
            parse_limits("250.00, 2730\n"),
            Some(Knobs {
                power_w: 250.0,
                clock_mhz: Some(2730.0)
            })
        );
    }

    #[test]
    fn refuses_a_limit_it_cannot_read() {
        assert_eq!(parse_limits("[N/A], 2730"), None, "[N/A] is not a limit");
        assert_eq!(
            parse_limits("250.00"),
            None,
            "a missing clock is not a stock clock"
        );
        assert_eq!(parse_limits(""), None);
    }

    #[test]
    fn a_silent_failure_still_says_something() {
        assert_eq!(first_line(b""), "nvidia-smi failed");
        assert_eq!(
            first_line(b"Insufficient Permissions\nmore"),
            "Insufficient Permissions"
        );
    }

    #[test]
    fn a_read_only_backend_never_writes() {
        let mut ro = ReadOnly::new("why");
        let k = Knobs {
            power_w: 1.0,
            clock_mhz: None,
        };
        assert!(ro.stock().is_err());
        assert!(ro.apply(k).is_err());
        assert!(
            ro.reset(k).is_ok(),
            "nothing changed, so nothing to restore"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn amd_sysfs_reads_and_writes_the_power_cap_in_microwatts() {
        let dir = std::env::temp_dir().join(format!("hub-amd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let cap = dir.join("power1_cap");
        std::fs::write(&cap, "250000000\n").expect("seed cap");
        let mut amd = AmdSysfs::new(cap.clone());
        let stock = amd.stock().expect("readable cap");
        assert_eq!(
            stock,
            Knobs {
                power_w: 250.0,
                clock_mhz: None
            }
        );
        amd.apply(Knobs {
            power_w: 150.0,
            clock_mhz: None,
        })
        .expect("write");
        assert_eq!(std::fs::read_to_string(&cap).expect("read"), "150000000");
        amd.reset(stock).expect("restore");
        assert_eq!(std::fs::read_to_string(&cap).expect("read"), "250000000");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
