//! Build-artifact accounting.
//!
//! This tree has filled its volume twice. Once `target/` consumes the last
//! byte, cargo fails with `os error 112` and rustc with `0xc0000409`, neither
//! of which says "the disk is full" — and a cold rebuild afterwards is 52
//! minutes on the machine this was written for. A number before the build is
//! worth more than a diagnosis after it.

use std::fs;
use std::path::Path;
use std::time::Instant;

/// Above this, `--check` fails. The foundation phase set it; it is a budget,
/// not a measurement of what is normal here.
const CEILING_BYTES: u64 = 30 * 1024 * 1024 * 1024;

/// Directories that hold build output. The nested workspaces each have their
/// own `target/`, because a crate with its own `[workspace]` table does not
/// share the root's.
const ARTIFACT_DIRS: &[&str] = &[
    "target",
    "target-contracts",
    "fuzz/target",
    "offsec-sandbox/target",
    "iot-firmware/target",
    "ebpf-net/programs/target",
    "dashboard/target",
    "app-maya2c/target",
    "contracts/token-swap/target",
    "wallet-gui/src-tauri/target",
    "wallet-gui/ui/target",
    "docs/site/node_modules",
];

pub fn run(args: &[String]) -> Result<(), String> {
    let check = args.iter().any(|a| a == "--check");
    let root = crate::workspace_root();
    let started = Instant::now();

    let mut rows: Vec<(String, u64)> = Vec::new();
    for rel in ARTIFACT_DIRS {
        let path = root.join(rel);
        if !path.exists() {
            continue;
        }
        eprintln!("  measuring {rel} ...");
        rows.push(((*rel).to_string(), dir_size(&path)));
    }
    rows.sort_by(|a, b| b.1.cmp(&a.1));

    let total: u64 = rows.iter().map(|(_, n)| n).sum();

    println!("\nBuild artifacts");
    println!("{:<34} {:>12}", "path", "size");
    println!("{:-<34} {:->12}", "", "");
    for (name, bytes) in &rows {
        println!("{name:<34} {:>12}", human(*bytes));
    }
    println!("{:-<34} {:->12}", "", "");
    println!("{:<34} {:>12}", "total", human(total));
    println!(
        "\nceiling {}  ({} measured in {:.1}s)",
        human(CEILING_BYTES),
        rows.len(),
        started.elapsed().as_secs_f64()
    );

    if let Some(free) = free_space(&root) {
        println!("free on this volume: {}", human(free));
        if free < CEILING_BYTES {
            println!(
                "\nWARNING: less free space than the ceiling. A cold rebuild of this \
                 workspace needs room for the whole of target/, and running out \
                 reports as `os error 112`, not as a full disk."
            );
        }
    }

    if total > CEILING_BYTES {
        let msg = format!(
            "build artifacts are {} , over the {} ceiling. `cargo clean` frees \
             the root target/; a cold rebuild after one takes about 52 minutes.",
            human(total),
            human(CEILING_BYTES)
        );
        if check {
            return Err(msg);
        }
        println!("\nWARNING: {msg}");
    }
    Ok(())
}

/// Recursive size, following no symlinks.
///
/// Unreadable entries are skipped rather than propagated: a partial number is
/// what this command is for, and a locked file mid-build should not turn a
/// warning into a failure.
fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(entry.path());
            } else if meta.is_file() {
                total += meta.len();
            }
        }
    }
    total
}

/// Free bytes on the volume holding `path`, when the platform offers it
/// cheaply. `None` is not an error — the size report above is the point.
fn free_space(path: &Path) -> Option<u64> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        wide.push(0);
        let mut free: u64 = 0;
        // SAFETY: `wide` is a NUL-terminated UTF-16 path that outlives the
        // call, and the three out-parameters are distinct valid `u64`s.
        // GetDiskFreeSpaceExW writes only through the pointers it is given.
        let ok = unsafe {
            let mut total: u64 = 0;
            let mut total_free: u64 = 0;
            get_disk_free_space_ex_w(wide.as_ptr(), &mut free, &mut total, &mut total_free) != 0
        };
        ok.then_some(free)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        None
    }
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    #[link_name = "GetDiskFreeSpaceExW"]
    fn get_disk_free_space_ex_w(
        directory: *const u16,
        free_bytes_available_to_caller: *mut u64,
        total_bytes: *mut u64,
        total_free_bytes: *mut u64,
    ) -> i32;
}

fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_rounds_to_the_unit_a_person_would_use() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(1024), "1.0 KiB");
        assert_eq!(human(289 * 1024 * 1024 * 1024), "289.0 GiB");
    }

    #[test]
    fn an_empty_directory_measures_zero_and_a_missing_one_is_skipped() {
        let dir = std::env::temp_dir().join("maya-xtask-disk-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        assert_eq!(dir_size(&dir), 0);
        fs::write(dir.join("a"), [0u8; 100]).expect("write");
        assert_eq!(dir_size(&dir), 100);
        assert_eq!(dir_size(&dir.join("does-not-exist")), 0);
        let _ = fs::remove_dir_all(&dir);
    }
}
