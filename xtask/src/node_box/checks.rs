//! Host checks for `node setup` and `node doctor`: free disk, RAM, the clock
//! and the node's ports. `evaluate` is pure over `Facts`; `gather` is the only
//! part that asks the OS.

use std::net::TcpListener;
use std::path::Path;
use std::process::Command;

const GIB: u64 = 1 << 30;
/// Operator policy floors, not measurements. The chain's disk growth is not
/// measured yet; raise these once it is (see docs/ecosystem/node-in-a-box-*.md).
pub const MIN_DISK_GIB: u64 = 20;
pub const MIN_RAM_GIB: u64 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Check {
    pub name: &'static str,
    pub level: Level,
    pub detail: String,
}

/// What the host reported. `None` means the OS could not answer; that is a
/// warning, not a pass. `ports_in_use: None` means the ports were not asked.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Facts {
    pub free_disk_bytes: Option<u64>,
    pub total_ram_bytes: Option<u64>,
    pub ports_in_use: Option<Vec<u16>>,
    pub clock_synced: Option<bool>,
}

fn floor_check(name: &'static str, bytes: Option<u64>, floor_gib: u64, what: &str) -> Check {
    match bytes {
        None => Check {
            name,
            level: Level::Warn,
            detail: format!("could not read {what}"),
        },
        Some(b) if b < floor_gib * GIB => Check {
            name,
            level: Level::Fail,
            detail: format!("{} GiB {what}, need {floor_gib}", b / GIB),
        },
        Some(b) => Check {
            name,
            level: Level::Pass,
            detail: format!("{} GiB {what}", b / GIB),
        },
    }
}

/// Turns host facts into one check per item, in a fixed order.
pub fn evaluate(facts: &Facts) -> Vec<Check> {
    let mut checks = vec![
        floor_check("disk", facts.free_disk_bytes, MIN_DISK_GIB, "free disk"),
        floor_check("ram", facts.total_ram_bytes, MIN_RAM_GIB, "RAM"),
    ];
    if let Some(busy) = &facts.ports_in_use {
        checks.push(if busy.is_empty() {
            Check {
                name: "ports",
                level: Level::Pass,
                detail: "node ports are free".to_owned(),
            }
        } else {
            let list: Vec<String> = busy.iter().map(u16::to_string).collect();
            Check {
                name: "ports",
                level: Level::Fail,
                detail: format!("in use: {} (another node is running here)", list.join(", ")),
            }
        });
    }
    checks.push(match facts.clock_synced {
        Some(true) => Check {
            name: "clock",
            level: Level::Pass,
            detail: "synchronised".to_owned(),
        },
        Some(false) => Check {
            name: "clock",
            level: Level::Fail,
            detail: "not synchronised; consensus timing needs a network time source".to_owned(),
        },
        None => Check {
            name: "clock",
            level: Level::Warn,
            detail: "could not read the time service".to_owned(),
        },
    });
    checks
}

/// The ports among `ports` something already listens on.
pub fn busy_ports(ports: &[u16]) -> Vec<u16> {
    ports
        .iter()
        .copied()
        .filter(|p| TcpListener::bind(("127.0.0.1", *p)).is_err())
        .collect()
}

/// `df -Pk`: the Available column (fourth) of the last line, in bytes.
pub fn parse_df_available_bytes(text: &str) -> Option<u64> {
    let line = text.lines().last()?;
    let kib: u64 = line.split_whitespace().nth(3)?.parse().ok()?;
    kib.checked_mul(1024)
}

/// `/proc/meminfo`: `MemTotal:   16318976 kB`, in bytes.
pub fn parse_meminfo_total_bytes(text: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    kib.checked_mul(1024)
}

/// `timedatectl show -p NTPSynchronized --value`.
pub fn parse_timedatectl_synced(text: &str) -> Option<bool> {
    match text.trim() {
        "yes" => Some(true),
        "no" => Some(false),
        _ => None,
    }
}

/// `w32tm /query /status`: a `Source:` of the local clock means it is not
/// synchronised to anything; any other source is a network time service.
pub fn parse_w32tm_synced(text: &str) -> Option<bool> {
    let source = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("Source:"))?
        .trim();
    Some(!(source.contains("Local CMOS") || source.contains("Free-running")))
}

fn run_text(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn powershell(script: &str) -> Option<String> {
    run_text(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", script],
    )
}

/// Free bytes on the volume holding `dir`.
fn free_disk(dir: &Path) -> Option<u64> {
    if cfg!(windows) {
        let quoted = dir.display().to_string().replace('\'', "''");
        let script = format!("[System.IO.DriveInfo]::new('{quoted}').AvailableFreeSpace");
        powershell(&script)?.trim().parse().ok()
    } else {
        let dir = dir.display().to_string();
        parse_df_available_bytes(&run_text("df", &["-Pk", &dir])?)
    }
}

fn total_ram() -> Option<u64> {
    if cfg!(windows) {
        let text = powershell("(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory")?;
        text.trim().parse().ok()
    } else {
        parse_meminfo_total_bytes(&std::fs::read_to_string("/proc/meminfo").ok()?)
    }
}

fn clock_synced() -> Option<bool> {
    if cfg!(windows) {
        parse_w32tm_synced(&run_text("w32tm", &["/query", "/status"])?)
    } else {
        parse_timedatectl_synced(&run_text(
            "timedatectl",
            &["show", "-p", "NTPSynchronized", "--value"],
        )?)
    }
}

/// Asks the OS. `ports` of `None` skips the port check (`node doctor`, where
/// the node is expected to hold its own ports).
pub fn gather(dir: &Path, ports: Option<&[u16]>) -> Facts {
    Facts {
        free_disk_bytes: free_disk(dir),
        total_ram_bytes: total_ram(),
        ports_in_use: ports.map(busy_ports),
        clock_synced: clock_synced(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn healthy() -> Facts {
        Facts {
            free_disk_bytes: Some(100 * GIB),
            total_ram_bytes: Some(16 * GIB),
            ports_in_use: Some(vec![]),
            clock_synced: Some(true),
        }
    }

    fn level(checks: &[Check], name: &str) -> Level {
        checks.iter().find(|c| c.name == name).unwrap().level
    }

    #[test]
    fn a_healthy_host_passes_every_check() {
        let checks = evaluate(&healthy());
        assert!(checks.iter().all(|c| c.level == Level::Pass), "{checks:?}");
    }

    #[test]
    fn disk_below_the_floor_fails_and_says_how_much_is_missing() {
        let facts = Facts {
            free_disk_bytes: Some((MIN_DISK_GIB - 1) * GIB),
            ..healthy()
        };
        let checks = evaluate(&facts);
        assert_eq!(level(&checks, "disk"), Level::Fail);
        assert!(checks[0].detail.contains("need"));
    }

    #[test]
    fn ram_at_exactly_the_floor_passes() {
        let facts = Facts {
            total_ram_bytes: Some(MIN_RAM_GIB * GIB),
            ..healthy()
        };
        assert_eq!(level(&evaluate(&facts), "ram"), Level::Pass);
    }

    #[test]
    fn an_unreadable_disk_is_a_warning_not_a_pass() {
        let facts = Facts {
            free_disk_bytes: None,
            ..healthy()
        };
        assert_eq!(level(&evaluate(&facts), "disk"), Level::Warn);
    }

    #[test]
    fn a_busy_node_port_fails_the_ports_check() {
        let facts = Facts {
            ports_in_use: Some(vec![30333]),
            ..healthy()
        };
        let checks = evaluate(&facts);
        assert_eq!(level(&checks, "ports"), Level::Fail);
        assert!(checks.iter().any(|c| c.detail.contains("30333")));
    }

    #[test]
    fn ports_that_were_not_asked_for_produce_no_ports_check() {
        let facts = Facts {
            ports_in_use: None,
            ..healthy()
        };
        assert!(evaluate(&facts).iter().all(|c| c.name != "ports"));
    }

    #[test]
    fn an_unsynchronised_clock_fails_and_an_unknown_one_warns() {
        let off = Facts {
            clock_synced: Some(false),
            ..healthy()
        };
        assert_eq!(level(&evaluate(&off), "clock"), Level::Fail);
        let unknown = Facts {
            clock_synced: None,
            ..healthy()
        };
        assert_eq!(level(&evaluate(&unknown), "clock"), Level::Warn);
    }

    #[test]
    fn a_bound_port_is_busy_and_a_free_one_is_not() {
        // Why: the fake "port in use" is a real listener, so the check sees what the OS sees.
        let held = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = held.local_addr().unwrap().port();
        assert_eq!(busy_ports(&[port]), vec![port]);
        drop(held);
        assert!(busy_ports(&[port]).is_empty());
    }

    #[test]
    fn df_output_gives_the_available_column_in_bytes() {
        let text = "Filesystem     1024-blocks      Used Available Capacity Mounted on\n/dev/sda1        1000    200    800      20% /\n";
        assert_eq!(parse_df_available_bytes(text), Some(800 * 1024));
    }

    #[test]
    fn meminfo_gives_the_total_in_bytes() {
        let text = "MemFree:  1 kB\nMemTotal:       16318976 kB\n";
        assert_eq!(parse_meminfo_total_bytes(text), Some(16_318_976 * 1024));
        assert_eq!(parse_meminfo_total_bytes("MemFree: 1 kB\n"), None);
    }

    #[test]
    fn timedatectl_reads_yes_and_no_only() {
        assert_eq!(parse_timedatectl_synced("yes\n"), Some(true));
        assert_eq!(parse_timedatectl_synced("no\n"), Some(false));
        assert_eq!(parse_timedatectl_synced("garbage"), None);
    }

    #[test]
    fn w32tm_treats_the_local_clock_as_unsynchronised() {
        let local = "Leap Indicator: 0(no warning)\nSource: Local CMOS Clock\n";
        let network = "Source: time.windows.com,0x9\n";
        assert_eq!(parse_w32tm_synced(local), Some(false));
        assert_eq!(parse_w32tm_synced(network), Some(true));
        assert_eq!(parse_w32tm_synced("no source line"), None);
    }
}
