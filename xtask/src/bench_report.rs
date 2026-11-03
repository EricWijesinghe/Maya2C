//! `cargo xtask bench-report` — criterion's JSON to CSV and Markdown tables.
//!
//! Reads `target/criterion/*/*/new/estimates.json`, criterion's own output, so
//! the report carries the numbers criterion measured rather than a
//! transcription of them. Writes `docs/benchmarks/algo_comparison.{csv,md}`.
//!
//! Energy is modelled, not measured: every joule figure is `measured seconds x
//! ASSUMED_PACKAGE_WATTS`, and the assumption is printed in the report beside
//! the results. There is deliberately no energy-per-transaction column — it
//! needs network hashrate and a hardware mix, and `Maya2C` has no live network.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::pyfmt;

const CRITERION: &str = "target/criterion";
const CSV_OUT: &str = "docs/benchmarks/algo_comparison.csv";
const MD_OUT: &str = "docs/benchmarks/algo_comparison.md";

/// Package power for the joules-per-operation model. The first input a reader
/// should challenge; kept identical to `crates/node/benches/algo_comparison.rs`.
const ASSUMED_PACKAGE_WATTS: f64 = 45.0;

struct Measurement {
    group: String,
    name: String,
    nanos: f64,
}

impl Measurement {
    fn seconds(&self) -> f64 {
        self.nanos / 1e9
    }

    fn joules(&self) -> f64 {
        self.seconds() * ASSUMED_PACKAGE_WATTS
    }

    fn per_second(&self) -> f64 {
        let s = self.seconds();
        if s > 0.0 { 1.0 / s } else { f64::INFINITY }
    }
}

pub fn run(_args: &[String]) -> ExitCode {
    match report() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => {
            println!("no criterion output found; run `cargo bench --bench algo_comparison` first");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("bench_report: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `Ok(false)` when criterion left nothing behind; nothing is written then.
fn report() -> Result<bool, String> {
    let root = crate::workspace_root();
    let rows = collect(&root.join(CRITERION))?;
    if rows.is_empty() {
        return Ok(false);
    }
    write_csv(&root.join(CSV_OUT), &rows)?;
    write_markdown(&root.join(MD_OUT), &rows)?;
    println!("{} measurements -> {CSV_OUT} and {MD_OUT}", rows.len());
    let mut sorted: Vec<&Measurement> = rows.iter().collect();
    sorted.sort_by(|a, b| a.group.cmp(&b.group).then(cmp_nanos(a.nanos, b.nanos)));
    for row in sorted {
        println!(
            "  {:<24} {:<34} {:>12}",
            row.group,
            row.name,
            humanise(row.seconds())
        );
    }
    Ok(true)
}

fn cmp_nanos(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
}

/// Every completed measurement criterion left behind, in path order.
fn collect(criterion: &Path) -> Result<Vec<Measurement>, String> {
    let mut found = Vec::new();
    for estimates in estimate_files(criterion) {
        // .../criterion/<group>/<bench>/new/estimates.json
        let bench_dir = estimates
            .parent()
            .and_then(Path::parent)
            .ok_or("bad criterion path")?;
        let group = bench_dir
            .parent()
            .and_then(Path::file_name)
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let name = bench_dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let text =
            fs::read_to_string(&estimates).map_err(|e| format!("{}: {e}", estimates.display()))?;
        let data: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", estimates.display()))?;
        // The median, not the mean: a scheduler hiccup during a background
        // compile lands in the long tail of a mean but not in the median.
        let Some(nanos) = data
            .pointer("/median/point_estimate")
            .and_then(serde_json::Value::as_f64)
        else {
            continue;
        };
        found.push(Measurement {
            group: group.to_string(),
            name: name.to_string(),
            nanos,
        });
    }
    Ok(found)
}

/// `<criterion>/*/*/new/estimates.json`, sorted. Dot-directories count: the
/// Python `Path.glob` this replaces does not skip them.
fn estimate_files(criterion: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for group in subdirs(criterion) {
        for bench in subdirs(&group) {
            let file = bench.join("new").join("estimates.json");
            if file.is_file() {
                found.push(file);
            }
        }
    }
    found.sort();
    found
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    out.sort();
    out
}

/// A duration in the unit a reader can hold in their head.
fn humanise(seconds: f64) -> String {
    if seconds >= 1e-3 {
        format!("{} ms", pyfmt::fixed(seconds * 1e3, 3))
    } else if seconds >= 1e-6 {
        format!("{} us", pyfmt::fixed(seconds * 1e6, 3))
    } else {
        format!("{} ns", pyfmt::fixed(seconds * 1e9, 1))
    }
}

fn ensure_parent(path: &Path) -> Result<(), String> {
    match path.parent() {
        Some(dir) => fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display())),
        None => Ok(()),
    }
}

fn write_csv(path: &Path, rows: &[Measurement]) -> Result<(), String> {
    ensure_parent(path)?;
    let watts = pyfmt::repr(ASSUMED_PACKAGE_WATTS);
    let mut out = pyfmt::csv_row(&[
        "group",
        "algorithm",
        "median_ns",
        "ops_per_second",
        "modelled_joules_per_op",
        "assumed_watts",
    ]);
    for row in rows {
        out.push_str(&pyfmt::csv_row(&[
            row.group.as_str(),
            row.name.as_str(),
            &pyfmt::fixed(row.nanos, 1),
            &pyfmt::fixed(row.per_second(), 2),
            &pyfmt::fixed(row.joules(), 9),
            watts.as_str(),
        ]));
    }
    fs::write(path, out).map_err(|e| format!("{}: {e}", path.display()))
}

fn write_markdown(path: &Path, rows: &[Measurement]) -> Result<(), String> {
    ensure_parent(path)?;
    let mut groups: BTreeMap<&str, Vec<&Measurement>> = BTreeMap::new();
    for row in rows {
        groups.entry(row.group.as_str()).or_default().push(row);
    }
    let mut lines: Vec<String> = intro().into_iter().map(str::to_string).collect();
    for (group, members) in &mut groups {
        lines.push(format!("## {group}"));
        lines.push(String::new());
        lines.push("| algorithm | median | ops/s | modelled J/op |".to_string());
        lines.push("|---|---:|---:|---:|".to_string());
        members.sort_by(|a, b| cmp_nanos(a.nanos, b.nanos));
        for row in members.iter() {
            lines.push(format!(
                "| {} | {} | {} | {} |",
                row.name,
                humanise(row.seconds()),
                pyfmt::grouped(row.per_second(), 1),
                pyfmt::exp(row.joules(), 3),
            ));
        }
        lines.push(String::new());
        // A ratio is the only cross-row arithmetic that means anything, and
        // only within a group.
        if let (Some(fastest), Some(slowest)) = (members.first(), members.last())
            && members.len() >= 2
        {
            let ratio = if fastest.nanos == 0.0 {
                f64::INFINITY
            } else {
                slowest.nanos / fastest.nanos
            };
            lines.push(format!(
                "`{}` is **{}x** the cost of `{}` in this group.",
                slowest.name,
                pyfmt::grouped(ratio, 0),
                fastest.name,
            ));
            lines.push(String::new());
        }
    }
    fs::write(path, lines.join("\n")).map_err(|e| format!("{}: {e}", path.display()))
}

/// The fixed preamble of the Markdown report: what each number is and is not.
fn intro() -> Vec<&'static str> {
    vec![
        "# Comparative benchmark results",
        "",
        "Generated by `cargo xtask bench-report` from criterion's own JSON.",
        "Re-run with `cargo bench --bench algo_comparison`.",
        "",
        "## Read this before the numbers",
        "",
        "**Two categories, not one.** SHA-256 is a proof-of-work hash; Ed25519 and",
        "ML-DSA+SLH-DSA are signature schemes. They are tabulated separately and no",
        "figure is compared across them. Note also that **Bitcoin does not use",
        "Ed25519** — its signatures are ECDSA over secp256k1, and Schnorr since",
        "Taproot. Ed25519 is here as the fast classical baseline, not as Bitcoin's.",
        "",
        "**Energy is modelled, not measured.** Every joule figure below is",
        "`measured seconds x 45.0 W`, and that wattage is an",
        "assumption. RAPL is not readable without a driver on this platform, so",
        "package power is an input, not a reading. Challenge it first.",
        "",
        "**There is no energy-per-transaction figure.** That needs network hashrate",
        "and a hardware mix, neither observable — published Bitcoin estimates carry",
        "roughly a factor-of-two spread for this reason. Maya2C has no live network,",
        "so its hashrate is zero and the quotient is undefined.",
        "",
        "**No `finalized` metric either.** Maya2C and Bitcoin have probabilistic",
        "finality: there is no finalisation event, only confirmations accumulating.",
        "",
        "**kHeavyHash (Kaspa) is timed in `benches/crypto.rs`, not here.** The earlier",
        "refusal said there were no official vectors to check a reimplementation",
        "against; that was wrong -- rusty-kaspa publishes known answers for",
        "`heavy_hash` and for matrix generation. `benches/support/kheavyhash.rs` is a",
        "port of upstream's code, checked against both answers and against cSHAKE256",
        "re-derived from NIST SP 800-185 (`tests/kheavyhash_reference_tests.rs`). It",
        "is a reference, not Kaspa's miner: upstream uses an assembly Keccak and real",
        "miners use GPUs. This harness keeps its original four-way comparison.",
        "",
        "**Single machine.** These are one host's numbers. Nothing here supports a",
        "cross-machine claim.",
        "",
    ]
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::humanise;

    #[test]
    fn humanise_picks_the_readable_unit() {
        assert_eq!(humanise(0.0025), "2.500 ms");
        assert_eq!(humanise(0.000_002_5), "2.500 us");
        assert_eq!(humanise(0.000_000_002_5), "2.5 ns");
    }
}
