//! GPU telemetry from `nvidia-smi`, the tool every NVIDIA driver installs.
//!
//! Every number shown is one the driver reported; a field the driver does not
//! report (a fanless laptop GPU, say) is `None` and the UI draws a dash, never
//! a guess. Other vendors report nothing yet, and the UI says so.
//!
//! The tool is run by an absolute system path (`driver_tool.rs`), never by a
//! `PATH` search: ADR-137 S-8.

use std::process::Command;

use serde::Serialize;

/// The query, in the column order [`parse`] expects.
const QUERY: &str = "index,name,temperature.gpu,power.draw,power.limit,utilization.gpu,\
                     fan.speed,memory.used,memory.total,clocks.sm,uuid,\
                     power.default_limit,power.min_limit,driver_version";
const COLUMNS: usize = 14;

/// One adapter's live readings.
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct GpuStat {
    pub index: u32,
    /// The driver's device UUID, the key a tuning profile is saved under
    /// (autotune.rs). `None` when the driver reports none: such a GPU is shown,
    /// and it cannot be tuned.
    pub uuid: Option<String>,
    pub name: String,
    pub temp_c: Option<f64>,
    pub power_w: Option<f64>,
    pub power_limit_w: Option<f64>,
    pub util_pct: Option<f64>,
    pub fan_pct: Option<f64>,
    pub mem_used_mib: Option<f64>,
    pub mem_total_mib: Option<f64>,
    pub clock_mhz: Option<f64>,
    /// The driver's default power limit: the top of the tuner's range and the
    /// value every rollback returns to (ADR-137 HW-1).
    pub power_default_w: Option<f64>,
    /// The lowest power limit the driver accepts: the bottom of the range.
    pub power_min_w: Option<f64>,
    /// A profile is discarded when this changes.
    pub driver: Option<String>,
}

/// What the hub could learn about the GPUs.
#[derive(Clone, Debug, Default, Serialize)]
pub struct GpuReport {
    pub devices: Vec<GpuStat>,
    /// Why `devices` is empty, in words for the UI.
    pub note: Option<String>,
}

/// A reading, or `None` for the driver's "[N/A]" and friends.
fn number(field: &str) -> Option<f64> {
    field.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Parses `nvidia-smi --format=csv,noheader,nounits` output. Lines with the
/// wrong column count are skipped rather than guessed at.
#[must_use]
pub fn parse(csv: &str) -> Vec<GpuStat> {
    csv.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split(',').map(str::trim).collect();
            if f.len() != COLUMNS {
                return None;
            }
            Some(GpuStat {
                index: f[0].parse().ok()?,
                uuid: Some(f[10].to_string()).filter(|u| !u.is_empty()),
                name: f[1].to_string(),
                temp_c: number(f[2]),
                power_w: number(f[3]),
                power_limit_w: number(f[4]),
                util_pct: number(f[5]),
                fan_pct: number(f[6]),
                mem_used_mib: number(f[7]),
                mem_total_mib: number(f[8]),
                clock_mhz: number(f[9]),
                power_default_w: number(f[11]),
                power_min_w: number(f[12]),
                driver: Some(f[13].to_string()).filter(|d| !d.is_empty()),
            })
        })
        .collect()
}

/// Each adapter's reported temperature as `(index, °C)`. An adapter with no
/// reading is left out, so the watchdog never acts on a guess.
#[must_use]
pub fn temperatures(report: &GpuReport) -> Vec<(u32, f64)> {
    report
        .devices
        .iter()
        .filter_map(|g| Some((g.index, g.temp_c?)))
        .collect()
}

/// Reads every NVIDIA adapter now.
#[must_use]
pub fn read() -> GpuReport {
    let Some(tool) = crate::driver_tool::nvidia_smi() else {
        return GpuReport {
            devices: Vec::new(),
            note: Some(
                "No NVIDIA driver tool at its system path. AMD and Intel telemetry is not wired yet."
                    .into(),
            ),
        };
    };
    let output = Command::new(tool)
        .arg(format!("--query-gpu={QUERY}"))
        .arg("--format=csv,noheader,nounits")
        .output();
    match output {
        Ok(out) if out.status.success() => {
            let devices = parse(&String::from_utf8_lossy(&out.stdout));
            let note = devices
                .is_empty()
                .then(|| "nvidia-smi ran but reported no adapters".to_string());
            GpuReport { devices, note }
        }
        Ok(_) => GpuReport {
            devices: Vec::new(),
            note: Some("nvidia-smi failed; is the NVIDIA driver loaded?".into()),
        },
        Err(_) => GpuReport {
            devices: Vec::new(),
            note: Some("The NVIDIA driver tool could not be started.".into()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_real_shaped_line_and_keeps_missing_fields_missing() {
        let csv = "0, NVIDIA GeForce RTX 5070, 61, 182.40, 250.00, 97, [N/A], 9120, 12227, 2700, GPU-aaaa, 250.00, 100.00, 581.29\n";
        let gpus = parse(csv);
        assert_eq!(gpus.len(), 1);
        let g = &gpus[0];
        assert_eq!(g.name, "NVIDIA GeForce RTX 5070");
        assert_eq!(g.uuid.as_deref(), Some("GPU-aaaa"));
        assert_eq!(g.temp_c, Some(61.0));
        assert_eq!(g.power_w, Some(182.4));
        assert_eq!(g.fan_pct, None, "[N/A] is not zero");
        assert_eq!(g.mem_total_mib, Some(12227.0));
        assert_eq!(g.power_default_w, Some(250.0));
        assert_eq!(g.power_min_w, Some(100.0));
        assert_eq!(g.driver.as_deref(), Some("581.29"));
    }

    #[test]
    fn the_query_names_one_field_per_column_with_no_spaces() {
        assert!(!QUERY.contains(char::is_whitespace), "{QUERY}");
        assert_eq!(QUERY.split(',').count(), COLUMNS);
    }

    #[test]
    fn a_short_or_garbled_line_is_skipped_not_guessed() {
        assert!(parse("0, GPU, 60\n").is_empty());
        assert!(parse("x, GPU, 1, 2, 3, 4, 5, 6, 7, 8, GPU-x, 9, 10, 11\n").is_empty());
        assert!(
            parse("0, GPU, 1, 2, 3, 4, 5, 6, 7, 8, GPU-x\n").is_empty(),
            "the old eleven-column query is refused"
        );
        assert!(parse("").is_empty());
    }

    #[test]
    fn two_adapters_keep_their_own_indices_and_a_blank_temperature_stays_blank() {
        let csv = "0, RTX A, 61, 182.40, 250.00, 97, 54, 9120, 12227, 2700, GPU-a, 250, 100, 581.29\n\
                   1, RTX B, [N/A], 90.10, 200.00, 0, [N/A], 512, 8192, 210, , [N/A], [N/A], 581.29\n";
        let gpus = parse(csv);
        assert_eq!(gpus.iter().map(|g| g.index).collect::<Vec<_>>(), [0, 1]);
        let report = GpuReport {
            devices: gpus,
            note: None,
        };
        assert_eq!(temperatures(&report), [(0, 61.0)]);
        assert_eq!(report.devices[1].uuid, None, "an empty UUID is no UUID");
        assert_eq!(
            report.devices[1].power_min_w, None,
            "an unreported range stays unreported"
        );
    }

    #[test]
    fn a_nan_or_infinite_reading_is_no_reading() {
        assert_eq!(number("NaN"), None);
        assert_eq!(number("inf"), None);
        assert_eq!(number(" 42.5 "), Some(42.5));
    }
}
